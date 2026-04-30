import pathlib

p = pathlib.Path(".github/workflows/CI.yml")
content = p.read_text(encoding="utf-8")

# Remove ENV FORCE_JAVASCRIPT_ACTIONS_TO_NODE24
content = content.replace("env:\n  FORCE_JAVASCRIPT_ACTIONS_TO_NODE24: true\n\n", "")

# Replace SHAs
replacements = {
    "actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v4.2.2": "actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2",
    "actions/setup-python@0b93645e9fea7318ecaed2b359559ac225c90a2b # v5.3.0": "actions/setup-python@a309ff8b426b58ec0e2a45f0f869d46889d02405 # v6.2.0",
    "actions/upload-artifact@b4b15b8c7c6ac21ea08fcf65892d2ee8f75cf882 # v4.4.3": "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1",
    "actions/download-artifact@fa0a91b85d4f404e444e00e005971372dc801d16 # v4.1.8": "actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c # v8.0.1"
}

for old, new in replacements.items():
    content = content.replace(old, new)

p.write_text(content, encoding="utf-8")
