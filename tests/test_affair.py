import struct
import zipfile
from unittest.mock import MagicMock, patch

import pytest

from mitdx import affair
from mitdx.affair import Affair

@pytest.mark.network
def test_affair_files():
    # Verify we can list remote TDX financial files
    files = Affair.files()
    assert len(files) > 0
    assert 'filename' in files[0]
    assert 'hash' in files[0]

    # Verify the specific list we expect is there (gpcw files)
    has_gpcw = any('gpcw' in f['filename'] for f in files)
    assert has_gpcw is True

def test_affair_parse_requires_filename():
    with pytest.raises(ValueError, match="Filename must be provided"):
        Affair.parse(downdir='.', filename=None)


# ---------------------------------------------------------------------------
# Offline regression tests (F7: disconnect on error, F12: path traversal,
# F20: download caps + malformed .dat validation). No network access: the
# GP client factory is always mocked.
# ---------------------------------------------------------------------------

HEADER_FMT = '<1hI1H3L'
ITEM_FMT = '<6s1c1L'
HEADER_SIZE = struct.calcsize(HEADER_FMT)  # 20
ITEM_SIZE = struct.calcsize(ITEM_FMT)      # 11


def build_dat(report_date=20240331, report_size=8, items=None, payload=b''):
    """Synthesize a gpcw-style .dat: header + stock index table + payload.

    items: list of (code_bytes, payload_offset); defaults to one stock whose
    payload starts right after a single-entry index table.
    """
    if items is None:
        items = [(b'600000', HEADER_SIZE + ITEM_SIZE)]
    # report_size is field index 4 (mootdx reads stock_header[4])
    header = struct.pack(HEADER_FMT, 0, report_date, len(items), 0, report_size, 0)
    table = b''.join(struct.pack(ITEM_FMT, code, b'\x00', off) for code, off in items)
    return header + table + payload


def mock_client(side_effect):
    client = MagicMock()
    client.get_report_file.side_effect = side_effect
    return client


# --- F12: filename validation ------------------------------------------------

UNSAFE_NAMES = [
    '../../evil.zip',
    'a/b.zip',
    '..\\x',
    'gpcw..zip',        # '..' substring is rejected even mid-name
    'evil\x00.zip',
    '..',
    '.',
    '/abs.zip',
    'C:\\evil.zip',
    'sub\\dir.zip',
]


@pytest.mark.parametrize('name', UNSAFE_NAMES)
def test_fetch_rejects_unsafe_filename(tmp_path, name):
    with patch('mitdx.affair._get_connected_client') as conn:
        with pytest.raises(ValueError, match='Unsafe financial filename'):
            Affair.fetch(downdir=str(tmp_path), filename=name)
    conn.assert_not_called()  # rejected before any network or disk I/O
    assert list(tmp_path.iterdir()) == []


@pytest.mark.parametrize('name', UNSAFE_NAMES[:5])
def test_parse_rejects_unsafe_filename_before_io(tmp_path, name):
    with patch('mitdx.affair._get_connected_client') as conn, \
         patch('mitdx.affair.Affair._parse_file') as parse_file:
        with pytest.raises(ValueError, match='Unsafe financial filename'):
            Affair.parse(downdir=str(tmp_path), filename=name)
    conn.assert_not_called()
    parse_file.assert_not_called()


@pytest.mark.parametrize('name', ['gpcw20240331.zip', 'gpcw.txt'])
def test_fetch_legit_filename_writes_file(tmp_path, name):
    payload = b'PK\x03\x04fakezip' if name.endswith('.zip') else b'gpcw20240331.zip,hash,123\n'
    client = mock_client([(len(payload), payload), (0, b'')])
    with patch('mitdx.affair._get_connected_client', return_value=client):
        assert Affair.fetch(downdir=str(tmp_path), filename=name) is True
    assert (tmp_path / name).read_bytes() == payload
    client.get_report_file.assert_any_call(f'tdxfin/{name}', 0)
    client.disconnect.assert_called_once()


def test_fetch_requires_filename():
    with pytest.raises(NotImplementedError, match='Batch downloading'):
        Affair.fetch(downdir='.', filename=None)


# --- F7: disconnect always runs ----------------------------------------------

def test_files_disconnects_on_fetch_error():
    client = mock_client(RuntimeError('boom'))
    with patch('mitdx.affair._get_connected_client', return_value=client):
        with pytest.raises(RuntimeError, match='boom'):
            Affair.files()
    client.disconnect.assert_called_once()


def test_fetch_disconnects_on_fetch_error(tmp_path):
    client = mock_client(RuntimeError('boom'))
    with patch('mitdx.affair._get_connected_client', return_value=client):
        with pytest.raises(RuntimeError, match='boom'):
            Affair.fetch(downdir=str(tmp_path), filename='gpcw20240331.zip')
    client.disconnect.assert_called_once()
    assert not (tmp_path / 'gpcw20240331.zip').exists()


def test_files_parses_listing_offline():
    listing = b'gpcw20240331.zip,abc123,1234\ngpcw.txt,def456,56\n\nbadline\n'
    client = mock_client([(len(listing), listing), (0, b'')])
    with patch('mitdx.affair._get_connected_client', return_value=client):
        files = Affair.files()
    assert files == [
        {'filename': 'gpcw20240331.zip', 'hash': 'abc123', 'filesize': 1234},
        {'filename': 'gpcw.txt', 'hash': 'def456', 'filesize': 56},
    ]
    client.disconnect.assert_called_once()


# --- F20: download loop caps and chunk sanity --------------------------------

def test_fetch_endless_stream_hits_round_cap(tmp_path):
    # Endless 1-byte chunks: only the round cap can stop this.
    client = mock_client(lambda *a: (1, b'x'))
    with patch('mitdx.affair._get_connected_client', return_value=client):
        with pytest.raises(IOError, match=r'exceeded \d+ request rounds'):
            Affair.fetch(downdir=str(tmp_path), filename='gpcw20240331.zip')
    assert client.get_report_file.call_count == affair._MAX_DOWNLOAD_ROUNDS
    client.disconnect.assert_called_once()
    assert not (tmp_path / 'gpcw20240331.zip').exists()


def test_fetch_byte_cap(tmp_path, monkeypatch):
    monkeypatch.setattr(affair, '_MAX_DOWNLOAD_BYTES', 100)
    client = mock_client(lambda *a: (60, b'a' * 60))
    with patch('mitdx.affair._get_connected_client', return_value=client):
        with pytest.raises(IOError, match=r'exceeded \d+ bytes'):
            Affair.fetch(downdir=str(tmp_path), filename='gpcw20240331.zip')
    assert client.get_report_file.call_count == 2  # 60 + 60 > 100
    client.disconnect.assert_called_once()


def test_fetch_short_chunk_is_eof(tmp_path):
    client = mock_client([(100, b'abc')])  # len(data) < chunksize -> EOF
    with patch('mitdx.affair._get_connected_client', return_value=client):
        Affair.fetch(downdir=str(tmp_path), filename='gpcw.txt')
    assert (tmp_path / 'gpcw.txt').read_bytes() == b'abc'
    assert client.get_report_file.call_count == 1
    client.disconnect.assert_called_once()


def test_fetch_oversized_chunk_rejected(tmp_path):
    client = mock_client([(2, b'abcdef')])  # more data than declared
    with patch('mitdx.affair._get_connected_client', return_value=client):
        with pytest.raises(IOError, match='chunksize'):
            Affair.fetch(downdir=str(tmp_path), filename='gpcw.txt')
    client.disconnect.assert_called_once()
    assert not (tmp_path / 'gpcw.txt').exists()


def test_fetch_filesize_stops_download():
    client = mock_client([(5, b'aaaaa'), (5, b'bbbbb'), (5, b'ccccc')])
    data = affair._fetch_file_content(client, 'tdxfin/gpcw.txt', 10)
    assert data == b'aaaaabbbbb'
    assert client.get_report_file.call_count == 2


# --- F20: malformed .dat headers ----------------------------------------------

def one_item_dat(report_size=8, max_count=1, offset=None, payload=None):
    if offset is None:
        offset = HEADER_SIZE + ITEM_SIZE
    if payload is None:
        payload = struct.pack('<2f', 1.5, 2.5)
    header = struct.pack(HEADER_FMT, 0, 20240331, max_count, 0, report_size, 0)
    item = struct.pack(ITEM_FMT, b'600000', b'\x00', offset)
    return header + item + payload


MALFORMED_CASES = [
    (b'', 'truncated header'),
    (b'\x00' * 10, 'truncated header'),
    # absurd report_size (~4 GiB) in a 39-byte file
    (one_item_dat(report_size=0xFFFFFFF0), 'report_size'),
    (one_item_dat(report_size=7), 'report_size'),   # not a multiple of 4
    (one_item_dat(report_size=0), 'report_size'),
    # max_count=65535 but only header + one item present
    (one_item_dat(max_count=65535), 'index table'),
    # two declared stocks, only one index entry on disk
    (struct.pack(HEADER_FMT, 0, 20240331, 2, 0, 8, 0)
     + struct.pack(ITEM_FMT, b'600000', b'\x00', HEADER_SIZE + 2 * ITEM_SIZE)
     + struct.pack('<2f', 1.5, 2.5), 'index table'),
    # payload offset far beyond EOF
    (one_item_dat(offset=1000), 'exceeds file size'),
    # payload truncated: offset 31 + 8 bytes needed, only 4 present
    (one_item_dat(payload=struct.pack('<f', 1.5)), 'exceeds file size'),
]


@pytest.mark.parametrize('data,match', MALFORMED_CASES)
def test_parse_malformed_dat(tmp_path, data, match):
    (tmp_path / 'bad.dat').write_bytes(data)
    with patch('mitdx.affair._get_connected_client') as conn:
        with pytest.raises(ValueError, match=match):
            Affair.parse(downdir=str(tmp_path), filename='bad.dat')
    conn.assert_not_called()  # local file exists; no download attempted


# --- happy path: parser still works on well-formed data -----------------------

def test_parse_synthetic_dat(tmp_path):
    dat = build_dat(payload=struct.pack('<2f', 1.5, 2.5))
    (tmp_path / 'gpcw20240331.dat').write_bytes(dat)
    with patch('mitdx.affair._get_connected_client') as conn:
        df = Affair.parse(downdir=str(tmp_path), filename='gpcw20240331.dat')
    conn.assert_not_called()
    assert df.index.tolist() == ['600000']
    assert df.index.name == 'code'
    assert len(df.columns) == 3
    assert df.columns[0] == 'report_date'
    assert df.iloc[0, 0] == 20240331
    assert df.iloc[0, 1] == pytest.approx(1.5)
    assert df.iloc[0, 2] == pytest.approx(2.5)


def test_parse_synthetic_dat_multiple_stocks(tmp_path):
    base = HEADER_SIZE + 2 * ITEM_SIZE  # payload starts after 2-entry table
    dat = build_dat(
        report_size=4,
        items=[(b'600000', base), (b'000001', base + 4)],
        payload=struct.pack('<2f', 1.5, 2.5),
    )
    (tmp_path / 'gpcw20240331.dat').write_bytes(dat)
    df = Affair.parse(downdir=str(tmp_path), filename='gpcw20240331.dat')
    assert df.index.tolist() == ['600000', '000001']
    assert df.iloc[0, 1] == pytest.approx(1.5)
    assert df.iloc[1, 1] == pytest.approx(2.5)
    assert (df['report_date'] == 20240331).all()


def test_parse_synthetic_zip_cleans_tmpdir(tmp_path):
    dat = build_dat(payload=struct.pack('<2f', 1.5, 2.5))
    with zipfile.ZipFile(tmp_path / 'gpcw20240331.zip', 'w') as zf:
        zf.writestr('gpcw20240331.dat', dat)
    extract_dir = tmp_path / 'extracted'
    with patch('mitdx.affair.tempfile.mkdtemp', return_value=str(extract_dir)):
        df = Affair.parse(downdir=str(tmp_path), filename='gpcw20240331.zip')
    assert df.index.tolist() == ['600000']
    assert df.iloc[0, 1] == pytest.approx(1.5)
    assert not extract_dir.exists()  # finally-path cleanup ran


def test_parse_empty_dat(tmp_path):
    dat = build_dat(items=[], payload=b'')  # max_count=0
    (tmp_path / 'empty.dat').write_bytes(dat)
    df = Affair.parse(downdir=str(tmp_path), filename='empty.dat')
    assert len(df) == 0
    assert list(df.columns) == ['code', 'report_date']


def test_parse_aggregate_cell_cap(tmp_path, monkeypatch):
    """Final-review fix: max_count * fields is bounded in aggregate.

    Every index entry may point at the same valid payload, so per-entry
    offset checks alone would still allow max_count * fields floats.
    """
    monkeypatch.setattr(affair, "_MAX_FINANCIAL_CELLS", 4)
    # 3 stocks x 2 fields = 6 cells > cap of 4 -> rejected before allocation.
    payload = struct.pack('<2f', 1.0, 2.0)
    off = HEADER_SIZE + 3 * ITEM_SIZE
    items = [(b'600000', off), (b'600001', off), (b'600002', off)]
    dat = build_dat(report_size=8, items=items, payload=payload)
    (tmp_path / 'big.dat').write_bytes(dat)
    with pytest.raises(ValueError, match='cell safety cap'):
        Affair.parse(downdir=str(tmp_path), filename='big.dat')
