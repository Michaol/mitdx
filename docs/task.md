# mitdx 重写任务清单

## 已完成

- [x] 深度代码审查 mootdx
- [x] KISS + Karpathy 头脑风暴 (sequential-thinking)
- [x] 查询 Context7: PyO3 + Maturin 文档
- [x] 读取 superpowers / brainstorming / karpathy-guidelines 技能

## Phase 1: 基础设施 + 本地二进制解析 ✅

- [x] maturin init + 混合项目结构搭建
- [x] Rust 日线解析器 (src/reader.rs → read_daily_bars)
- [x] Rust 分钟线解析器 (src/reader.rs → read_minute_bars)
- [x] Python reader.py 向后兼容封装
- [x] pytest 7/7 通过 (合成二进制文件验证)
- [x] git init + 首次提交

## Phase 2: 网络层 — TCP 协议 + 服务器发现 ✅

- [x] Rust TDX HQ 协议实现
- [x] tokio TCP 连接池 (使用 KISS 原则简化为标准库 synchronous TcpStream)
- [x] 并发服务器测速 (Rust std::thread 高性能并发测速)
- [x] Python quotes.py 封装

## Phase 3: 财务数据 + 交易日历 ✅

- [x] Rust 财务数据下载解析 (TDX TCP协议高速下载 + Python内置ZIP/struct解析)
- [x] 内置中国交易日历 (纯Python动态解析TDX隐藏官方CSV接口，抛弃笨重静态内置与V8)
- [x] 移除 py_mini_racer 依赖 (完全消除，无需任何外部JS引擎)

## Phase 4: 新功能 + 发布 ✅

- [x] Polars 支持
- [x] 完整 .pyi 类型桩
- [x] 跨平台 CI/CD
- [x] PyPI 发布
