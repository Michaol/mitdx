import tempfile
import shutil
import struct
from pathlib import Path
from typing import Optional, List, Dict, Union, Tuple, Any

from mitdx._core import TdxClient
from mitdx.columns import columns as COLUMNS
from mitdx.consts import GP_HOSTS
from mitdx.utils import to_df

def _get_connected_client() -> TdxClient:
    client = TdxClient()
    for name, ip, port in GP_HOSTS:
        try:
            if client.connect(ip, port):
                return client
        except (OSError, ConnectionError):
            pass
    raise ConnectionError("Failed to connect to any TDX Financial (GP) Server to fetch affairs")

def _fetch_file_content(client: TdxClient, filename: str, filesize: int = 0) -> bytes:
    offset = 0
    buf = bytearray()
    while True:
        chunksize, data = client.get_report_file(filename, offset)
        if chunksize == 0 or not data:
            break
        buf.extend(data)
        offset += chunksize  # use the chunksize requested increment
        if len(data) < chunksize:  # server returned less than requested, EOF
            break
        if filesize > 0 and offset >= filesize:
            break
    return bytes(buf)

class Affair:
    @staticmethod
    def files() -> List[Dict[str, Union[str, int]]]:
        """Fetch list of all available financial files on TDX"""
        client = _get_connected_client()
        data = _fetch_file_content(client, "tdxfin/gpcw.txt", 0)
        client.disconnect()

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
        downdir_path = Path(downdir)
        downdir_path.mkdir(parents=True, exist_ok=True)

        if not filename:
            raise NotImplementedError("Batch downloading all files is currently not supported. Provide 'filename'.")

        client = _get_connected_client()
        data = _fetch_file_content(client, f"tdxfin/{filename}", 0)
        client.disconnect()

        dest = downdir_path / filename
        dest.write_bytes(data)
        return True

    @staticmethod
    def parse(downdir: str = '.', filename: Optional[str] = None, backend: str = 'pandas', **kwargs) -> Any:
        """Fetch and parse financial zip/dat into DataFrame"""
        if not filename:
            raise ValueError("Filename must be provided")

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
        dat_fp = None

        try:
            if filepath.suffix == '.zip':
                tmpdir = Path(tempfile.mkdtemp(prefix='mitdx_'))
                shutil.unpack_archive(filepath, extract_dir=tmpdir)
                for file in tmpdir.iterdir():
                    if file.suffix == '.dat':
                        dat_fp = open(file, 'rb')
                        break
                if not dat_fp:
                    raise FileNotFoundError(f"No .dat file found in zip archive: {filepath}")
            elif filepath.suffix == '.dat':
                dat_fp = open(filepath, 'rb')
            else:
                raise ValueError(f"File must be .zip or .dat, got: {filepath.suffix}")

            with dat_fp:
                data_header = dat_fp.read(header_size)
                stock_header = struct.unpack(header_pack_format, data_header)

                max_count = stock_header[2]
                report_date = stock_header[1]
                report_size = stock_header[4]

                report_fields_count = int(report_size / 4)
                report_pack_format = f'<{report_fields_count}f'

                results = []
                for stock_idx in range(max_count):
                    dat_fp.seek(header_size + stock_idx * stock_item_size)
                    stock_item = struct.unpack('<6s1c1L', dat_fp.read(stock_item_size))
                    code = stock_item[0].decode('utf-8')

                    dat_fp.seek(stock_item[2])
                    info_data = dat_fp.read(struct.calcsize(report_pack_format))
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
