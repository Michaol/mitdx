# mitdx

**通达信数据高性能读取接口** — Rust 核心 + Python 包装混合架构

[![PyPI](https://img.shields.io/pypi/v/mitdx.svg)](https://pypi.org/project/mitdx/)
[![Python](https://img.shields.io/pypi/pyversions/mitdx.svg)](https://pypi.org/project/mitdx/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

## 简介

`mitdx` 是 [mootdx](https://github.com/mootdx/mootdx) 的下一代重写版本，采用 **Rust + Python** 混合架构，通过 [PyO3](https://pyo3.rs) / [Maturin](https://maturin.rs) 构建。

| 特性              | 说明                                                   |
| ----------------- | ------------------------------------------------------ |
| 🚀 **极速解析**   | Rust `memmap2` 内存映射 + 零拷贝解析 VIPDOC 二进制文件 |
| 🔌 **实时行情**   | Rust 原生 TCP 网络层，支持 TDX HQ 协议获取实时 K 线    |
| 📊 **财务数据**   | 从 TDX 服务器下载并解析 `.dat` / `.zip` 财报数据       |
| 📅 **交易日历**   | 中国市场假日判断，自动缓存 + 网络降级                  |
| 🛠️ **命令行工具** | 内置 `mitdx` CLI，一行命令快速获取行情与数据           |
| 🐼 **多后端支持** | 返回 Pandas 格式，或超高性能的 Polars DataFrame        |
| 📦 **跨平台构建** | CI/CD 自动构建全平台 Wheel，无需本地 C/C++ 编译环境    |
| 🔄 **向后兼容**   | API 设计兼容 mootdx，支持平滑迁移                      |

## 安装

```bash
pip install mitdx
```

**从源码安装（开发者）：**

```bash
pip install maturin
git clone https://github.com/your-org/mitdx.git && cd mitdx
maturin develop --release
```

## 快速开始

### 1. 离线数据读取（Reader）

读取本地通达信安装目录下的 VIPDOC 二进制文件。

```python
from mitdx.reader import Reader

# 创建标准 A 股市场读取器，指定通达信安装目录
reader = Reader.factory(market='std', tdxdir='C:/new_tdx')

# --- 日线数据 ---
df = reader.daily(symbol='600036')
print(df)
#              open   high    low  close      amount   volume
# date
# 2024-01-02  33.50  33.81  33.25  33.76  628359168.0  1864.0
# ...

# --- 1 分钟线 ---
df = reader.minute(symbol='600036', suffix=1)

# --- 5 分钟线 ---
df = reader.fzline(symbol='600036')
# 等价于 reader.minute(symbol='600036', suffix=5)
```

> **说明：** `symbol` 支持纯代码 (`600036`)、带前缀代码 (`sh600036`)，自动推断交易所。

**证券类型自动识别：** 根据文件名自动匹配 A 股、B 股、科创板、基金、债券、指数的价格系数。

### 2. 实时行情（Quotes）

通过 TDX HQ 协议从行情服务器获取实时 K 线数据。

```python
from mitdx.quotes import Quotes

# 方式一：自动选择最快服务器（推荐）
with Quotes.factory(market='std') as q:
    # 默认返回 pandas DataFrame，也可以指定 backend='polars'
    df = q.bars(symbol='600036', frequency=9, count=20, backend='polars')
    print(df)

# 方式二：手动指定服务器
q = Quotes(market='std')
q.connect(ip='110.41.147.114', port=7709)
df = q.bars(symbol='600036', frequency=9, start=0, count=100)
q.disconnect()
```

**频率参数 `frequency` 说明：**

| 值   | 含义    | 值      | 含义             |
| ---- | ------- | ------- | ---------------- |
| `0`  | 5 分钟  | `5`     | 15 分钟          |
| `1`  | 15 分钟 | `6`     | 30 分钟          |
| `2`  | 30 分钟 | `7`     | 1 小时           |
| `3`  | 1 小时  | `8`     | 日线             |
| `4`  | 日线    | **`9`** | **日线（默认）** |
| `10` | 周线    | `11`    | 月线             |

### 3. 财务数据（Affair）

从 TDX 金融服务器下载并解析上市公司财务报告。

```python
from mitdx.affair import Affair

# 列出所有可用的财务数据文件
files = Affair.files()
print(files)
# [{'filename': 'gpcw20240331.zip', 'hash': '...', 'filesize': 12345}, ...]

# 下载指定文件到本地目录
Affair.fetch(downdir='./data', filename='gpcw20240331.zip')

# 下载 + 解析为 DataFrame（自动中文列名）
df = Affair.parse(downdir='./data', filename='gpcw20240331.zip', backend='polars')
print(df.columns[:5])
# ['report_date', '基本每股收益', '扣除非经常性损益每股收益', '每股未分配利润', '每股净资产']
```

### 4. 交易日历（Calendar）

判断指定日期是否为中国 A 股交易日，假日数据从 TDX 官网自动获取并缓存 7 天。

```python
import datetime
from mitdx.calendar import is_trading_day, is_holiday

# 判断今天
print(is_trading_day())       # True / False
print(is_holiday())           # False / True

# 判断指定日期
d = datetime.date(2026, 1, 1)
print(is_holiday(d))          # True  — 元旦
print(is_trading_day(d))      # False
```

### 5. 服务器延迟测试

使用 Rust 多线程并发 ping 所有已知 TDX 行情服务器。

```python
from mitdx._core import ping_servers
from mitdx.consts import HQ_HOSTS

# 并发 ping 所有预配置服务器
ips = [(ip, port) for name, ip, port in HQ_HOSTS]
results = ping_servers(ips)

# 结果按延迟升序排列 [(ip, port, latency_ms), ...]
for ip, port, ms in results:
    print(f"{ip}:{port} → {ms}ms")
```

### 6. CLI 命令行工具

通过 `pip install` 安装后，可直接在终端使用 `mitdx` 命令行进行快捷操作：

```bash
# 获取实时行情 (使用 polars 后端)
mitdx quotes --symbol 600036 --start 0 --count 10 --backend polars

# 列出并下载财务文件
mitdx affair list
mitdx affair fetch --filename gpcw.txt

# 读取离线 VIPDOC 数据
mitdx reader daily --symbol 600036 --tdxdir C:/new_tdx
```

### 7. Rust 底层接口

直接使用 Rust 核心模块，获得最大性能控制。

```python
from mitdx._core import read_daily_bars, read_minute_bars, TdxClient

# 读取本地 .day 文件（可自定义价格系数）
records = read_daily_bars('C:/new_tdx/vipdoc/sh/lday/sh600036.day',
                          price_coeff=0.01, vol_coeff=0.01)
# records: [{'date': '2024-01-02', 'open': 33.5, 'high': 33.81, ...}, ...]

# 读取本地 .lc1 / .lc5 分钟线文件
records = read_minute_bars('C:/new_tdx/vipdoc/sh/minline/sh600036.lc1')

# TDX 网络客户端
client = TdxClient()
client.connect('110.41.147.114', 7709)
bars = client.get_security_bars(category=9, market=1, code='600036', start=0, count=10)
client.disconnect()
```

## 架构

```
mitdx/
├── src/                     # Rust 核心（PyO3 扩展模块）
│   ├── lib.rs               # 模块入口，导出 Python 接口
│   ├── reader.rs            # VIPDOC 二进制文件解析（mmap 零拷贝）
│   ├── network.rs           # TDX HQ 网络协议客户端
│   ├── protocol.rs          # 变长价格编码 / 自定义浮点解码
│   └── server.rs            # 多线程服务器 ping
├── python/mitdx/            # Python 包装层
│   ├── cli.py               # 命令行工具主程序
│   ├── reader.py            # 离线数据读取 API（StdReader / ExtReader）
│   ├── quotes.py            # 实时行情 API（自动服务器选优）
│   ├── affair.py            # 财务数据下载与解析
│   ├── calendar.py          # 交易日历（假日判断 + 缓存）
│   ├── columns.py           # 财务报表 580+ 列名定义
│   ├── consts.py            # 行情服务器地址
│   └── utils.py             # DataFrame (Pandas/Polars) 转换工具
├── Cargo.toml               # Rust 依赖
└── pyproject.toml           # Python 项目配置（Maturin 构建）
```

## 开发

### 环境要求

- Python ≥ 3.9
- Rust ≥ 1.70（[安装 Rust](https://rustup.rs)）
- Maturin ≥ 1.12

### 构建与测试

```bash
# 创建虚拟环境
python -m venv .venv && .venv\Scripts\activate  # Windows
# python -m venv .venv && source .venv/bin/activate  # Linux/macOS

# 安装开发依赖
pip install maturin pytest pandas polars

# 编译 Rust 扩展并安装到当前 venv
maturin develop --release

# 运行测试
pytest tests/ -v

# 仅检查 Rust 编译
cargo check
```

### 发布构建

```bash
# 构建 wheel
maturin build --release

# 产物位于 target/wheels/
```

## 从 mootdx 迁移

| mootdx                                     | mitdx                             | 变化               |
| ------------------------------------------ | --------------------------------- | ------------------ |
| `from mootdx.reader import Reader`         | `from mitdx.reader import Reader` | 仅包名             |
| `Reader.factory(market='std', tdxdir=...)` | 相同                              | 无变化             |
| `reader.daily(symbol='600036')`            | 相同                              | 无变化             |
| `Quotes.factory(market='std')`             | 相同                              | 无变化             |
| `q.bars(symbol=..., offset=10)`            | `q.bars(symbol=..., count=10)`    | `offset` → `count` |

## License

MIT
