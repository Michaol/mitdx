# mitdx

通达信 (TDX) 市场数据接口，Rust 核心 + Python 封装。

[![PyPI](https://img.shields.io/pypi/v/mitdx.svg)](https://pypi.org/project/mitdx/)
[![Python](https://img.shields.io/pypi/pyversions/mitdx.svg)](https://pypi.org/project/mitdx/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Quality Gate Status](https://sonarcloud.io/api/project_badges/measure?project=Michaol_mitdx&metric=alert_status)](https://sonarcloud.io/summary/new_code?id=Michaol_mitdx)

## Changelog

### v1.1.4 (2026-04-30)

代码审查全量修复：Rust 协议层 market 参数统一为 `u8` 并修正 `get_transaction_data` 数据包长度；修复 `reader.rs` 分钟线日期解码与 `protocol.rs` 不一致；`affair.py` 文件句柄泄漏修复；`calendar.py` 网络失败增加日志警告；所有网络测试标记 `@pytest.mark.network`，CI 增加 test job。

<details>
<summary>历史版本</summary>

**v1.1.3 (2026-04-30)**

修复 SonarCloud 安全审查项：测试文件移除硬编码 IP（改为引用 `consts.py`）；CI 收紧 tag 触发规则和权限声明。

**v1.1.2 (2026-04-30)**

文档修正，与 PyPI 同步。

**v1.1.1 (2026-04-30)**

修复 `finance()` 语义错误：改为调用独立的 `get_finance_info`（CMD `0x0010`），返回 34 字段财务摘要。

**v1.1.0 (2026-04-30)**

新增 5 个 Rust 原生协议解析器，Python 层对齐全部 mootdx 核心 API。

| 新增协议                    | 指令      | 功能                        |
| --------------------------- | --------- | --------------------------- |
| `get_xdxr_info`             | `0x000F`  | 除权除息                    |
| `get_transaction_data`      | `0x0FC5`  | 分笔成交                    |
| `get_security_quotes`       | `0x5053E` | 实时五档盘口                |
| `get_company_info_category` | `0x02CF`  | F10 公司资料目录            |
| `get_company_info_content`  | `0x02D0`  | F10 公司资料内容 (GBK→UTF8) |

新增 Python API：`xdxr()` · `transactions()` · `quotes()` · `finance()` · `f10()` · `minute()` · `index()`

其他：新增 `encoding_rs` 依赖处理 GBK；统一使用 `read_response()` / `require_stream()` 消除重复代码；`_market_code()` 改用 `removeprefix()` 替代 `lstrip()`。

**v1.0.0 (2026-04-29)**

首个正式版本。Rust 核心实现 K 线解析、VIPDOC 文件读取、TDX HQ 协议。Python 层提供 Reader / Quotes / Affair / Calendar 四个模块，CLI 工具，Pandas / Polars 双后端，CI/CD 跨平台 Wheel 构建。

</details>

## 安装

PyPI（发布后可用）：

```bash
pip install mitdx
```

从 GitHub Release 安装预编译 Wheel：

```bash
pip install mitdx --find-links https://github.com/Michaol/mitdx/releases/latest
```

从源码构建：

```bash
pip install maturin
git clone https://github.com/Michaol/mitdx.git && cd mitdx
maturin develop --release
```

## 用法

### Reader — 离线 VIPDOC 文件读取

```python
from mitdx.reader import Reader

reader = Reader.factory(market='std', tdxdir='C:/new_tdx')

df = reader.daily(symbol='600036')
df = reader.minute(symbol='600036', suffix=1)   # 1分钟线
df = reader.fzline(symbol='600036')              # 5分钟线
```

`symbol` 支持纯代码 (`600036`) 或带前缀 (`sh600036`)，自动推断交易所。

### Quotes — 实时行情

```python
from mitdx.quotes import Quotes

with Quotes.factory(market='std') as q:
    df = q.bars(symbol='600036', frequency=9, count=20)           # K线
    df = q.minute(symbol='600036', frequency=0, count=20)         # 分钟线 (bars 别名)
    df = q.index(symbol='000001', frequency=9, count=20)          # 指数 (bars 别名)
    df = q.xdxr(symbol='600036')                                  # 除权除息
    df = q.transactions(symbol='600036', start=0, count=30)       # 分笔成交
    df = q.quotes(symbols=['600036', '000001'])                   # 实时快照 (批量)
    df = q.finance(symbol='600036')                               # 财务数据 (xdxr 别名)
    info = q.f10(symbol='600036')                                 # F10 公司资料
```

手动指定服务器：

```python
q = Quotes(market='std')
q.connect(ip='110.41.147.114', port=7709)
df = q.bars(symbol='600036', frequency=9, start=0, count=100)
q.disconnect()
```

所有方法均支持 `backend='polars'` 参数切换至 Polars DataFrame。

**`frequency` 参数：**

| 值   | 含义   | 值      | 含义            |
| ---- | ------ | ------- | --------------- |
| `0`  | 5分钟  | `5`     | 15分钟          |
| `1`  | 15分钟 | `6`     | 30分钟          |
| `2`  | 30分钟 | `7`     | 1小时           |
| `3`  | 1小时  | `8`     | 日线            |
| `4`  | 日线   | **`9`** | **日线 (默认)** |
| `10` | 周线   | `11`    | 月线            |

### Affair — 财务数据

```python
from mitdx.affair import Affair

files = Affair.files()                                                          # 文件列表
Affair.fetch(downdir='./data', filename='gpcw20240331.zip')                     # 下载
df = Affair.parse(downdir='./data', filename='gpcw20240331.zip')                # 解析
```

### Calendar — 交易日历

```python
import datetime
from mitdx.calendar import is_trading_day, is_holiday

is_trading_day()                        # 今天是否交易日
is_holiday(datetime.date(2026, 1, 1))   # 指定日期是否假日
```

### CLI

```bash
mitdx quotes --symbol 600036 --count 10 --backend polars
mitdx affair list
mitdx affair fetch --filename gpcw.txt
mitdx reader daily --symbol 600036 --tdxdir C:/new_tdx
```

### Rust 底层接口

```python
from mitdx._core import read_daily_bars, read_minute_bars, TdxClient

records = read_daily_bars('path/to/sh600036.day', price_coeff=0.01, vol_coeff=0.01)
records = read_minute_bars('path/to/sh600036.lc1')

client = TdxClient()
client.connect('110.41.147.114', 7709)
bars = client.get_security_bars(category=9, market=1, code='600036', start=0, count=10)
client.disconnect()
```

## 项目结构

```
mitdx/
├── src/                     # Rust (PyO3)
│   ├── lib.rs               # 模块入口
│   ├── reader.rs            # VIPDOC 解析 (mmap)
│   ├── network.rs           # TDX HQ 协议客户端
│   ├── protocol.rs          # 变长价格编码 / 浮点解码
│   └── server.rs            # 并发 ping
├── python/mitdx/            # Python
│   ├── quotes.py            # 实时行情
│   ├── reader.py            # 离线数据
│   ├── affair.py            # 财务数据
│   ├── calendar.py          # 交易日历
│   ├── cli.py               # 命令行工具
│   ├── consts.py            # 服务器地址
│   ├── columns.py           # 财报列名 (580+)
│   └── utils.py             # DataFrame 转换
├── Cargo.toml
└── pyproject.toml
```

## 开发

需要 Python ≥ 3.9、Rust ≥ 1.70、Maturin ≥ 1.12。

```bash
python -m venv .venv && .venv\Scripts\activate
pip install maturin pytest pandas polars
maturin develop --release
pytest tests/ -v
```

## 从 mootdx 迁移

| mootdx                                     | mitdx                             | 变化               |
| ------------------------------------------ | --------------------------------- | ------------------ |
| `from mootdx.reader import Reader`         | `from mitdx.reader import Reader` | 包名               |
| `Reader.factory(market='std', tdxdir=...)` | 相同                              | —                  |
| `reader.daily(symbol='600036')`            | 相同                              | —                  |
| `Quotes.factory(market='std')`             | 相同                              | —                  |
| `q.bars(symbol=..., offset=10)`            | `q.bars(symbol=..., count=10)`    | `offset` → `count` |

## License

MIT
