# AI 实现 Prompt 清单

本文件根据 `dev.md`、`rule.md`、`README.md` 整理，用于直接投喂给 AI coding agent。按顺序执行，控制在 20 个 prompt 内完成当前规划的主要需求。

通用要求：

- 每个 prompt 执行前先阅读 `dev.md`、`rule.md`、`README.md`。
- 遵守 Rust workspace 现有模块边界。
- UI 层只能调用 `app-core` 服务，不直接依赖数据库驱动。
- 所有数据库 IO、文件 IO、Redis 扫描、查询执行不得阻塞 UI 线程。
- 密码、token、完整连接字符串不得出现在日志、错误、调试输出中。
- 用户输入值必须参数绑定；标识符必须通过方言模块引用。
- 完成后运行可行的 `cargo fmt --all`、`cargo check --workspace`、相关测试。
- 行为变化必须同步更新 `README.md`、`dev.md` 或相关设计文档。

## 1. 项目基线审计

```text
请在当前 Rust workspace 中审计现有实现，对照 dev.md、rule.md、README.md，列出已完成、缺失、阻塞项。不要大规模重构，只修复会影响后续开发的编译、测试、模块边界和明显安全问题。完成后更新 README.md 的当前状态。
```

## 2. 桌面 UI 主框架

```text
基于 egui/eframe 实现桌面主界面三栏布局：顶部工具栏、左侧连接/对象树、中央多标签工作区、底部状态栏。UI 只能调用 app-core 服务，不直接依赖数据库驱动。加入统一 AppState、Tab 模型、任务状态模型和错误展示入口。
```

## 3. 连接管理 UI

```text
实现连接管理完整 UI：新建、编辑、删除、分组、最近使用、测试连接、保存连接。支持 Redis、MySQL、PostgreSQL、SQLite、DuckDB。密码默认不保存，只有用户选择保存时写入 secure-store。错误提示必须脱敏并可操作。
```

## 4. 连接会话和后台任务

```text
为 app-core 增加连接会话/任务编排层：所有数据库 IO、文件 IO、Redis 扫描、查询执行都不能阻塞 UI 线程。支持任务状态、取消、超时、连接状态显示和断线重连入口。同步驱动 SQLite/DuckDB 使用 blocking thread pool 封装。
```

## 5. 对象树 UI

```text
接入 MetadataService，实现左侧数据库对象树 UI。支持懒加载、单节点刷新、对象搜索、连接状态标记、右键菜单。关系型数据库展示 database/schema/table/view/column/index；Redis 展示 DB 编号和 key 概览。不得使用 Redis KEYS。
```

## 6. 查询编辑器

```text
实现多标签 SQL 查询编辑器 UI：新建查询、关闭确认、连接/schema 选择、执行当前语句、执行选中语句、停止执行、执行耗时、影响行数、多结果集展示。默认只执行当前语句，多语句执行必须有明确入口。
```

## 7. SQL 语句处理和历史收藏

```text
在 app-core 中实现 SQL 语句拆分、当前语句定位、查询历史、常用 SQL 收藏。历史和收藏持久化到本地配置目录。补充 SQL 拆分单元测试，覆盖分号、字符串、注释、空语句。
```

## 8. 结果集表格完善

```text
完善 egui 结果集表格：分页、虚拟滚动、列宽调整、选中单元格、复制单元格、复制行、复制单行带表头、复制当前页带表头、复制完整结果带表头、按列升序/降序/取消排序。NULL、空字符串、二进制、日期时间必须清晰区分。
```

## 9. 导出到剪贴板和文件

```text
把查询结果和表数据导出接入 import-export crate，支持 CSV、JSON、Excel 兼容格式。当前已有剪贴板导出时保留；新增文件保存流程、编码、分隔符、是否包含表头配置。导出必须从结果模型或数据库流读取，不得反解析 UI 文本。
```

## 10. 表数据分页查看

```text
实现表数据标签页：从对象树双击表打开，按数据库方言生成安全分页 SQL，默认限制 500 行，支持过滤、排序、刷新、复制、导出。标识符必须通过方言 quote_identifier，用户值必须参数绑定。
```

## 11. 表结构查看

```text
实现表结构标签页：字段名、类型、nullable、默认值、主键、注释、索引、外键、建表 SQL、刷新。复用现有元数据适配器，不让 UI 直接调用驱动。补齐缺失数据库的元数据能力和测试。
```

## 12. 表数据编辑后端

```text
在 app-core 实现表数据编辑服务：新增行、编辑单元格、删除行、dirty 状态、提交、回滚、根据主键生成 UPDATE/DELETE、事务提交、影响行数校验。无主键表默认只读；无 WHERE 的 UPDATE/DELETE 必须高风险确认。
```

## 13. 表数据编辑 UI

```text
接入表数据编辑 UI：可编辑单元格、新增行、删除行、提交/回滚按钮、待提交变更数量、关闭未提交提醒、提交错误定位。提交前展示变更摘要；影响 0 行提示冲突，影响大于 1 行回滚并提示危险。
```

## 14. Redis Key 浏览

```text
实现 Redis key 浏览 UI：DB 选择、SCAN 分页、pattern 搜索、key 类型、TTL、大小摘要、删除确认、刷新。大 value 默认只展示预览，加载完整内容必须由用户确认。
```

## 15. Redis 常用类型编辑

```text
实现 Redis String、Hash、List、Set、Sorted Set 的查看和基础编辑。支持新增 key、设置 TTL、删除 key、Hash field 增删改、List 分页和 push/pop/set、Set member 增删、ZSet score 编辑。所有删除操作必须确认。
```

## 16. Redis 命令行

```text
实现 Redis 命令标签页：输入命令、执行、展示返回值、历史记录、错误展示。危险命令如 FLUSHDB、FLUSHALL、DEL 多 key 必须二次确认。日志和错误中不得暴露敏感信息。
```

## 17. CSV 导入

```text
实现 CSV 导入到关系型表：文件选择、预览、自动识别表头、字段映射、类型预校验、批量写入、事务、失败停止或跳过、错误报告含行号和字段。覆盖空值、引号、换行、大文件、类型错误测试。
```

## 18. 安全和配置完善

```text
完善本地安全：连接密码只保存 password_ref，支持不保存密码、首次访问输入密码、只读连接配置、连接配置导出默认不含密码、配置文件权限检查、日志脱敏。补充错误脱敏和配置校验测试。
```

## 19. 体验增强和快捷键

```text
实现核心快捷键和体验：Ctrl/Cmd+N 新建查询、Ctrl/Cmd+Enter 执行当前 SQL、Ctrl/Cmd+Shift+Enter 执行选中 SQL、Ctrl/Cmd+S 保存/提交、Ctrl/Cmd+R 刷新、Ctrl/Cmd+F 搜索、Ctrl/Cmd+L 聚焦对象搜索、Esc 关闭弹窗或取消编辑。加入主题切换、SQL 格式化基础版、错误详情展开面板。
```

## 20. 最终验收和收尾

```text
对照 dev.md 第 20 节完成标准做最终收尾：修复遗漏功能、补齐关键单元测试和集成测试、确认 UI 不阻塞、大表/大量 Redis key 使用分页或流式策略、危险操作有确认、文档同步更新。最后运行 cargo fmt --all、cargo check --workspace、cargo test --workspace、cargo clippy --workspace --all-targets -- -D warnings，并输出未能运行或失败的原因。
```
