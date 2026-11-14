from __future__ import annotations

import os
import tempfile
import shutil
import struct
from contextlib import contextmanager
from pathlib import Path
from typing import Optional, List, Dict, Tuple, Any, Iterator

from mitdx._core import TdxClient
from mitdx.columns import columns as COLUMNS
from mitdx.consts import GP_HOSTS
from mitdx.utils import to_df

# Safety caps for server-driven downloads: a broken or malicious server must
# not be able to stream forever (infinite loop) or exhaust memory. The byte
# cap dominates for full-size chunks (~30000 bytes each); the round cap also
# stops servers that dribble out tiny chunks indefinitely.
_MAX_DOWNLOAD_BYTES = 512 * 1024 * 1024  # 512 MiB total per file
_MAX_DOWNLOAD_ROUNDS = 100_000  # max request/response rounds per file

# Aggregate cap on parsed financial cells (stocks x fields). Per-entry offset
# checks alone are not enough: every index entry may point at the SAME valid
# payload, so max_count * report_fields_count floats could still be allocated.
# Real gpcw files are ~6000 stocks x ~580 fields (~3.5M cells); 10M is generous.
_MAX_FINANCIAL_CELLS = 10_000_000

def _get_connected_client() -> TdxClient:
    client = TdxClient()
    for name, ip, port in GP_HOSTS:
        try:
            if client.connect(ip, port):
                return client
        except OSError:
            pass
    raise ConnectionError("Failed to connect to any TDX Financial (GP) Server to fetch affairs")

@contextmanager
def _connected_client() -> Iterator[TdxClient]:
    """Yield a connected GP client; disconnect even if the caller raises."""
    client = _get_connected_client()
    try:
        yield client
    finally:
        client.disconnect()

def _validate_filename(filename: str) -> None:
    """Reject filenames that could escape the download directory.

    Filenames may come from the server (via files()), so a name must be a
    single plain path component: no '/', '\\', '..' or null bytes, and equal
    to its own basename. 'gpcw..zip' is rejected as well — legitimate TDX
    filenames (gpcw20240331.zip, gpcw.txt) never contain '..'.
    """
    if (
        not isinstance(filename, str)
        or not filename
        or '/' in filename
        or '\\' in filename
        or '..' in filename
        or '\x00' in filename
        or Path(filename).name != filename
    ):
        raise ValueError(
            f"Unsafe financial filename (must be a plain name without path "
            f"separators, '..' or null bytes): {filename!r}"
        )

def _fetch_file_content(client: TdxClient, filename: str, filesize: int = 0) -> bytes:
    offset = 0
    rounds = 0
    buf = bytearray()
    while True:
        if rounds >= _MAX_DOWNLOAD_ROUNDS:
            raise IOError(
                f"Download of {filename!r} exceeded {_MAX_DOWNLOAD_ROUNDS} "
                f"request rounds ({len(buf)} bytes received); aborting to "
                f"avoid an endless transfer"
            )
        chunksize, data = client.get_report_file(filename, offset)
        rounds += 1
        if chunksize <= 0 or not data:
            break
        if len(data) > chunksize:
            raise IOError(
                f"Malformed response for {filename!r} at offset {offset}: "
                f"server declared chunksize {chunksize} but sent {len(data)} bytes"
            )
        if len(buf) + len(data) > _MAX_DOWNLOAD_BYTES:
            raise IOError(
                f"Download of {filename!r} exceeded {_MAX_DOWNLOAD_BYTES} bytes "
                f"(received {len(buf)} + {len(data)}); aborting"
            )
        buf.extend(data)
        offset += chunksize  # use the chunksize requested increment
        if len(data) < chunksize:  # server returned less than requested, EOF
            break
        if filesize > 0 and offset >= filesize:
            break
    return bytes(buf)

def _validate_dat_header(
    data_header: bytes,
    header_pack_format: str,
    header_size: int,
    file_size: int,
    stock_item_size: int,
    dat_path: Path,
) -> Tuple[int, int, int]:
    """Unpack and bounds-check the gpcw header; return (max_count, report_date, report_size).

    Header values come from the (untrusted) file: bound them against the
    actual file size before allocating/seeking. Extracted from _parse_file
    to keep both functions under the cognitive-complexity limit (S3776).
    """
    if len(data_header) != header_size:
        raise ValueError(
            f"Malformed financial file {dat_path}: truncated header "
            f"(expected {header_size} bytes, got {len(data_header)})"
        )
    stock_header = struct.unpack(header_pack_format, data_header)
    max_count = stock_header[2]
    report_date = stock_header[1]
    report_size = stock_header[4]

    # report_size is a float-array byte count: a positive multiple of 4
    # that fits in the file.
    if report_size == 0 or report_size % 4 != 0 or report_size > file_size:
        raise ValueError(
            f"Malformed financial file {dat_path}: invalid report_size "
            f"{report_size} (file is {file_size} bytes; must be a "
            f"positive multiple of 4 within the file)"
        )
    index_end = header_size + max_count * stock_item_size
    if index_end > file_size:
        raise ValueError(
            f"Malformed financial file {dat_path}: header declares "
            f"{max_count} stocks but the index table needs {index_end} "
            f"bytes (file is {file_size} bytes)"
        )
    # Aggregate bound: every index entry may point at the SAME valid payload,
    # so per-entry offset checks alone still allow max_count * fields floats.
    total_cells = max_count * (report_size // 4)
    if total_cells > _MAX_FINANCIAL_CELLS:
        raise ValueError(
            f"Malformed financial file {dat_path}: header declares "
            f"{max_count} stocks x {report_size // 4} fields "
            f"({total_cells} cells), exceeding the "
            f"{_MAX_FINANCIAL_CELLS}-cell safety cap"
        )
    return max_count, report_date, report_size

class Affair:
    @staticmethod
    def files() -> List[Dict[str, str | int]]:
        """Fetch list of all available financial files on TDX"""
        with _connected_client() as client:
            data = _fetch_file_content(client, "tdxfin/gpcw.txt", 0)

        content = data.decode('utf-8').strip().split('\n')
        results = []
        for line in content:
            if line.strip():
                parts = line.strip().split(',')
                if len(parts) >= 3:
                    results.append({'filename': parts[0], 'hash': parts[1], 'filesize': int(parts[2])})
        return results

    @staticmethod
    def fetch(downdir: str = '.', filename: Optional[str] = None):
        """Download financial zip file from TDX servers"""
        if not filename:
            raise NotImplementedError("Batch downloading all files is currently not supported. Provide 'filename'.")
        # Validate before any I/O: the name may come from the server and is
        # joined into the local write path below. basename() is a no-op after
        # validation but makes the sanitization visible to taint analysis.
        _validate_filename(filename)
        filename = os.path.basename(filename)

        downdir_path = Path(downdir)
        downdir_path.mkdir(parents=True, exist_ok=True)

        with _connected_client() as client:
            data = _fetch_file_content(client, f"tdxfin/{filename}", 0)

        dest = downdir_path / filename
        dest.write_bytes(data)
        return True

    @staticmethod
    def parse(downdir: str = '.', filename: Optional[str] = None, backend: str = 'pandas', **kwargs) -> Any:
        """Fetch and parse financial zip/dat into DataFrame"""
        if not filename:
            raise ValueError("Filename must be provided")
        _validate_filename(filename)
        filename = os.path.basename(filename)

        filepath = Path(downdir) / filename
        if not filepath.exists():
            Affair.fetch(downdir, filename)

        return Affair._parse_file(filepath, backend)

    @staticmethod
    def _parse_file(filepath: Path, backend: str = 'pandas') -> Any:
        header_pack_format = '<1hI1H3L'
        stock_item_size = struct.calcsize('<6s1c1L')
        header_size = struct.calcsize(header_pack_format)

        tmpdir = None
        try:
            if filepath.suffix == '.zip':
                tmpdir = Path(tempfile.mkdtemp(prefix='mitdx_'))
                shutil.unpack_archive(filepath, extract_dir=tmpdir)
                dat_file = next((f for f in tmpdir.iterdir() if f.suffix == '.dat'), None)
                if not dat_file:
                    raise FileNotFoundError(f"No .dat file found in zip archive: {filepath}")
                dat_path = dat_file
            elif filepath.suffix == '.dat':
                dat_path = filepath
            else:
                raise ValueError(f"File must be .zip or .dat, got: {filepath.suffix}")

            file_size = dat_path.stat().st_size
            with open(dat_path, 'rb') as dat_fp:
                data_header = dat_fp.read(header_size)
                max_count, report_date, report_size = _validate_dat_header(
                    data_header, header_pack_format, header_size,
                    file_size, stock_item_size, dat_path,
                )
                report_fields_count = report_size // 4
                report_pack_format = f'<{report_fields_count}f'

                results = []
                for stock_idx in range(max_count):
                    dat_fp.seek(header_size + stock_idx * stock_item_size)
                    item_data = dat_fp.read(stock_item_size)
                    if len(item_data) != stock_item_size:
                        raise ValueError(
                            f"Malformed financial file {dat_path}: truncated index "
                            f"entry for stock #{stock_idx} (expected {stock_item_size} "
                            f"bytes, got {len(item_data)})"
                        )
                    stock_item = struct.unpack('<6s1c1L', item_data)
                    code = stock_item[0].decode('utf-8')

                    payload_offset = stock_item[2]
                    if payload_offset + report_size > file_size:
                        raise ValueError(
                            f"Malformed financial file {dat_path}: stock {code!r} "
                            f"report offset {payload_offset} + report_size "
                            f"{report_size} exceeds file size {file_size}"
                        )
                    dat_fp.seek(payload_offset)
                    info_data = dat_fp.read(struct.calcsize(report_pack_format))
                    if len(info_data) != report_size:
                        raise ValueError(
                            f"Malformed financial file {dat_path}: truncated report "
                            f"payload for stock {code!r} (expected {report_size} "
                            f"bytes, got {len(info_data)})"
                        )
                    cw_info = struct.unpack(report_pack_format, info_data)

                    results.append((code, report_date) + cw_info)
        finally:
            if tmpdir:
                shutil.rmtree(tmpdir, ignore_errors=True)

        return Affair._to_df(results, backend)

    @staticmethod
    def _to_df(data: List[Tuple], backend: str = 'pandas') -> Any:
        if not data:
            return to_df([], columns=['code', 'report_date'], backend=backend)

        column = ['code', 'report_date']
        for i in range(1, len(data[0]) - 1):
            column.append('col' + str(i))

        df = to_df(data, columns=column, backend=backend)

        # Set Chinese headers if applicable
        col_names: List[str] = list(COLUMNS)
        if backend == 'pandas':
            df.set_index('code', inplace=True)
            for i, v in enumerate(df.columns):
                if i >= len(col_names):
                    col_names.append(v)
            df.columns = col_names[:len(df.columns)]
        else:
            current_cols = df.columns
            for i, v in enumerate(current_cols):
                if i >= len(col_names):
                    col_names.append(v)
            df.columns = col_names[:len(current_cols)]

        return df
