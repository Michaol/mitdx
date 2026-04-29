# mitdx

通达信数据高性能读取接口 — Rust + Python 混合架构

[![PyPI](https://img.shields.io/pypi/v/mitdx.svg)](https://pypi.org/project/mitdx/)

## 简介

`mitdx` 是 [mootdx](https://github.com/mootdx/mootdx) 的下一代重写版本，
采用 **Rust 核心 + Python 包装** 架构，通过 [PyO3](https://pyo3.rs) 和 [Maturin](https://maturin.rs) 构建。

**核心优势：**

- 🚀 **极速解析** — Rust 内存映射 + 零拷贝解析 VIPDOC 二进制文件
- 📦 **零依赖冲突** — 预编译 wheel，无需本地 C++ 编译环境
- 🔄 **向后兼容** — API 与 mootdx 保持一致，平滑迁移

## 安装

```bash
pip install mitdx
```

## 快速开始

```python
from mitdx.reader import Reader

# 读取本地通达信数据
reader = Reader.factory(market='std', tdxdir='C:/new_tdx')

# 日线数据
df = reader.daily(symbol='600036')

# 1分钟线
df = reader.minute(symbol='600036')

# 5分钟线
df = reader.fzline(symbol='600036')
```

## 开发

```bash
# 安装 Rust: https://rustup.rs
# 安装 maturin: pip install maturin

# 开发构建
maturin develop --release

# 运行测试
pytest tests/ -v
```

## License

MIT
