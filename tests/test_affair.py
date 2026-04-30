import pytest
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
