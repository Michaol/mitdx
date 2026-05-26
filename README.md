# mitdx

通达信 (TDX) 市场数据接口，Rust 核心 + Python 封装。

[![PyPI](https://img.shields.io/pypi/v/mitdx.svg)](https://pypi.org/project/mitdx/)
[![Python](https://img.shields.io/pypi/pyversions/mitdx.svg)](https://pypi.org/project/mitdx/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Quality Gate Status](https://sonarcloud.io/api/project_badges/measure?project=Michaol_mitdx&metric=alert_status)](https://sonarcloud.io/summary/new_code?id=Michaol_mitdx)

## 支持的 TDX 协议

| 协议                        | 指令      | 功能                | Python API          |
| --------------------------- | --------- | ------------------- | ------------------- |
| `get_security_count`        | `0x044E`  | 市场证券数量        | `stock_count()`     |
| `get_security_list`         | `0x0450`  | 证券列表 (代码/名称)| `stocks()` `stock_all()` |
| `get_security_bars`         | `0x052D`  | K 线数据            | `bars()` `minute()` |
| `get_index_bars`            | `0x052D`  | 指数K线 (含涨跌家数)| `index_bars()`      |
| `get_security_quotes`       | `0x5053E` | 实时五档盘口        | `quotes()`          |
| `get_transaction_data`      | `0x0FC5`  | 当日分笔成交        | `transactions()`    |
| `get_minute_time_data`      | `0x051D`  | 当日分时数据        | `minute_data()`     |
| `get_history_minute_time`   | `0x0FB4`  | 历史分时数据        | `history_minute_data()` |
| `get_history_transaction`   | `0x0FB5`  | 历史分笔成交        | `history_transactions()` |
| `get_xdxr_info`             | `0x000F`  | 除权除息            | `xdxr()`            |
| `get_finance_info`          | `0x0010`  | 财务摘要            | `finance()`         |
| `get_company_info_category` | `0x02CF`  | F10 公司资料目录    | `f10()`             |
| `get_company_info_content`  | `0x02D0`  | F10 公司资料内容    | `f10()`             |
| `get_block_info_meta`       | `0x02C5`  | 板块元数据          | `block_meta()`      |
| `get_block_info`            | `0x06B9`  | 板块成分股          | `block_info()`      |
| `get_report_file`           | `0x06B9`  | 报表文件下载        | —                   |

## Changelog

### v1.1.10 (2026-05-26)

安全修复：添加 `uv.lock` 锁定 Python 依赖版本（25 个包），解决依赖版本不可预测的安全警告。

### v1.1.9 (2026-05-26)

新增 7 个通达信标准 HQ 协议接口，补齐分时数据、历史数据、指数K线和板块数据能力。

| 新增协议                    | 指令     | 功能                                |
| --------------------------- | -------- | ----------------------------------- |
| `get_index_bars`            | `0x052D` | 指数K线 (含涨跌家数 up/down_count)  |
| `get_minute_time_data`      | `0x051D` | 当日分时数据                        |
| `get_history_minute_time`   | `0x0FB4` | 历史分时数据 (指定日期)             |
| `get_history_transaction`   | `0x0FB5` | 历史分笔成交 (指定日期)             |
| `get_block_info_meta`       | `0x02C5` | 板块元数据 (文件尺寸/哈希)          |
| `get_block_info`            | `0x06B9` | 板块成分股数据                      |

新增 Python API：`index_bars()` · `minute_data()` · `history_minute_data()` · `history_transactions()` · `block_meta()` · `block_info()` · `k()`

其他：`index()` 现为 `index_bars()` 别名，自动识别指数市场码 (000xxx/88xxxx/99xxxx→SH，399xxx→SZ)；新增 `_parse_block_data()` Python 板块文件解析器；`k()` 支持日期范围 K 线查询。

<details>
<summary>历史版本</summary>

**v1.1.8 (2026-05-25)**

新增 TDX 协议 `GetSecurityCount` / `GetSecurityList` 命令，支持从通达信服务器获取全量股票代码与名称映射。

| 新增协议              | 指令     | 功能                                |
| --------------------- | -------- | ----------------------------------- |
| `get_security_count`  | `0x044E` | 获取市场证券数量                    |
| `get_security_list`   | `0x0450` | 获取证券列表 (代码/名称/昨收等)     |

新增 Python API：`stock_count()` · `stocks()` · `stock_all()`

其他：Rust 层新增 `parse_u16_le()` / `parse_u32_le()` 辅助函数消除重复；Python 层新增 `MARKET_SH` / `MARKET_SZ` / `SECURITY_LIST_BATCH_SIZE` 常量；`stock_all()` 与 `stocks()` 分页逻辑去重；全量 `logger.exception()` 替换 `logger.error()` 保留堆栈跟踪；市场参数校验与 mootdx API 兼容。

**v1.1.7 (2026-04-30)**

修复 1.1.4/1.1.5 严重核心回归：重构后 `get_security_bars`、`get_company_info_category`、`get_transaction_data` 结构化请求时由包头数据长度与实际写入长度不匹配阻碍服务端响应导致的 EAGAIN Socket 超时挂起，现已将被强转为 `u8` 的 `market` 字段在底层通信层恢复回原生的 `u16` 二字节结构，对齐发包长度完美解决。
同步彻底修复 MacOS (aarch64) 版本 CI 构建时由于跨缓存损坏引起的 `sccache` 序列化警告反序列化报错（已在 Actions 环境静默绕过并配置禁用 sccache）。

**v1.1.4 (2026-04-30)**

代码审查全量修复：Rust 协议层 market 参数统一为 `u8` 并修正 `get_transaction_data` 数据包长度；修复 `reader.rs` 分钟线日期解码与 `protocol.rs` 不一致；`affair.py` 文件句柄泄漏修复；`calendar.py` 网络失败增加日志警告；所有网络测试标记 `@pytest.mark.network`，CI 增加 test job。

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
from mitdx.consts import MARKET_SH, MARKET_SZ

with Quotes.factory(market='std') as q:
    # 证券列表
    count = q.stock_count(market=MARKET_SH)                     # 市场证券数量
    df = q.stocks(market=MARKET_SH)                              # 单市场证券列表 (自动分页)
    df = q.stock_all()                                           # 沪深全量证券列表

    # 行情数据
    df = q.bars(symbol='600036', frequency=9, count=20)          # K线
    df = q.minute(symbol='600036', frequency=0, count=20)        # 分钟线 (bars 别名)
    df = q.index_bars(symbol='000001', frequency=9, count=20)    # 指数K线 (含涨跌家数)
    df = q.xdxr(symbol='600036')                                 # 除权除息
    df = q.transactions(symbol='600036', start=0, count=30)      # 当日分笔成交
    df = q.quotes(symbols=['600036', '000001'])                  # 实时快照 (批量)
    df = q.finance(symbol='600036')                              # 财务数据
    info = q.f10(symbol='600036')                                # F10 公司资料

    # 分时与历史数据
    df = q.minute_data(symbol='600036')                          # 当日分时数据
    df = q.history_minute_data(symbol='600036', date=20250520)   # 历史分时数据
    df = q.history_transactions(symbol='600036', date=20250520)  # 历史分笔成交

    # 板块数据
    meta = q.block_meta('block_zs.dat')                          # 板块元数据
    df = q.block_info('block_zs.dat')                            # 板块成分股

    # 日期范围K线
    df = q.k(symbol='600036', begin='20250501', end='20250520')  # 按日期范围查K线
```

手动指定服务器：

```python
q = Quotes(market='std')
q.connect(ip='110.41.147.114', port=7709)
df = q.bars(symbol='600036', frequency=9, start=0, count=100)
q.disconnect()
```

所有方法均支持 `backend='polars'` 参数切换至 Polars DataFrame。

**`stocks()` / `stock_all()` 返回字段：**

| 字段            | 类型   | 说明                                |
| --------------- | ------ | ----------------------------------- |
| `code`          | str    | 证券代码 (如 `600036`)              |
| `name`          | str    | 证券名称 (GBK→UTF-8)               |
| `volunit`       | int    | 最小交易单位 (通常 100)             |
| `decimal_point` | int    | 价格小数位数 (通常 2)               |
| `pre_close`     | float  | 昨收价                              |
| `market`        | int    | 市场代码 (仅 `stock_all()` 返回)    |

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
count = client.get_security_count(market=1)
stocks = client.get_security_list(market=1, start=0)
index = client.get_index_bars(category=9, market=1, code='000001', start=0, count=10)
minute = client.get_minute_time_data(market=1, code='600036')
hist_min = client.get_history_minute_time_data(market=1, code='600036', date=20250520)
hist_tx = client.get_history_transaction_data(market=1, code='600036', start=0, count=10, date=20250520)
meta = client.get_block_info_meta(block_file='block_zs.dat')
chunk = client.get_block_info(block_file='block_zs.dat', start=0, size=meta['size'])
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
│   ├── consts.py            # 服务器地址 / 常量
│   ├── columns.py           # 财报列名 (580+)
│   ├── utils.py             # DataFrame 转换
│   └── _core.pyi            # Rust 扩展类型标注
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

| mootdx                                       | mitdx                                     | 变化                            |
| -------------------------------------------- | ----------------------------------------- | ------------------------------- |
| `from mootdx.reader import Reader`           | `from mitdx.reader import Reader`         | 包名                            |
| `Reader.factory(market='std', tdxdir=...)`   | 相同                                      | —                               |
| `reader.daily(symbol='600036')`              | 相同                                      | —                               |
| `Quotes.factory(market='std')`               | 相同                                      | —                               |
| `q.bars(symbol=..., offset=10)`              | `q.bars(symbol=..., count=10)`            | `offset` → `count`              |
| `q.stock_count(market=MARKET_SH)`            | 相同                                      | —                               |
| `q.stocks(market=MARKET_SH)`                 | 相同                                      | —                               |
| `q.stock_all()`                              | 相同                                      | —                               |
| `q.index_bars(symbol='000001')`              | 相同                                      | —                               |
| `q.minute_data(symbol='600036')`             | `q.minute_data(symbol='600036')`          | —                               |
| `q.minutes(symbol, date='20250520')`         | `q.history_minute_data(symbol, date=20250520)` | 方法名                    |
| `q.transactions(symbol, date='20250520')`    | `q.history_transactions(symbol, date=20250520)` | 方法名                 |
| `q.block('block_zs.dat')`                    | `q.block_info('block_zs.dat')`            | 方法名                          |
| `from mootdx.consts import MARKET_SH`        | `from mitdx.consts import MARKET_SH`      | 包名                            |

## License

MIT
