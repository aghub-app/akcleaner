# Interactive clean：第二轮 Luna 契约

用户目标：漂亮、简洁的 CLI，勾选清理内容，第二次明确确认，执行所有选中且可清理的集成。负责人只审阅；Luna 写实现。

## 基线与共同边界

- 使用主工作目录 `/Users/akrc/Developer/akcleaner` 中已经验收但未提交的只读 MVP 作为基线。不得创建提交或推送。
- Agent C 负责清理 core，在现有 `luna-scan-core` worktree 实现；Agent B 负责 CLI，在 `luna-host-adapters` worktree 实现。
- B 获授权从主目录只读复制 Cargo.toml、Cargo.lock、README.md、src、tests、rules 到自己的 worktree，同步基线；这会替换 B 旧的独立验证脚手架，已被主目录集成版本包含。不得复制 .git、target，不得改主目录或 C worktree。
- C 自己的 worktree 已是同一基线。两者读取此任务书后开始；不要运行安装器或扫描/修改真实 HOME 的 agent 配置。
- 所有删除测试仅在测试创建的临时 home 内。禁止随便 rm 产品目录、跟随软链接或在 hook/MCP command 中执行任何命令。
- 保留现有 scan JSON / rules check 接口。无自动 --yes 快捷方式；交互清理必须在终端内确认，重定向 stdin 不允许绕过。
- 修改文件前检查用户变化；只在授权 worktree 和文件范围内工作。不要启动额外 subagent。

## 体验要求（B）

- `akcleaner` 无子命令和 `akcleaner clean` 进入交互；`clean --home <path> --rules-dir <path> [--product <id>]` 便于模拟和限定范围。
- 使用成熟 Rust 终端交互组件，例如 dialoguer + console（优先保持依赖少）。上下键移动、空格勾选、Enter 下一步、Esc/Ctrl-C 取消。产品/类型排序显示；选择具体 artifact，避免仅有一个删除全部按钮。
- 整体是工具型 app：小标题、留白、清晰层级，一个青色 accent；红色只用于最终 destructive 提示，弱化 home 路径前缀。不要大 ASCII logo、emoji、卡片堆砌、假进度条。
- 首屏显示 AKCleaner、扫描范围、发现数量；列出 `Superset · Claude Code · MCP / skill / hook` 与路径/事件等可识别位置，不展示内部 rule_id、raw command、env、token。
- 使用简洁中文交互文案。选项不要预选。candidate hook 可由用户显式选择，但必须标注「归属待核实」并在最终清单中继续标记；最终确认即为用户对这些候选项的明确移除授权。
- confirmed+modified、conflicting、unknown MCP 不可直接选删，展示「已修改 / 归属冲突 / 无法验证，已保留」。confirmed+unknown 的 skill_marker 仅允许删除那一个 SKILL.md，不扩展目录内容。
- 完成选择后调用 prepare，打印具体文件删除与配置条目移除清单、备份位置/机制以及跳过原因；需要包含选中的 candidate 数量。
- 最终只问一次默认 No 的 `确认删除以上 N 项？`。拒绝、Esc、Ctrl-C、空选择不写任何文件（包括备份）。准备计划本身不产生副作用。
- 确认后执行，报告移除条目/文件数、失败/跳过数、备份路径。失败不得显示全部成功；完整/部分失败返回非零。部分扫描诊断应显示，但不必拦截不受影响的可验证项目。
- `NO_COLOR`、非 TTY 输出禁色；终端宽度较窄仍可读，路径不要不可逆截断；所有来自文件名/key 的终端控制字符转义后显示，不能注入 ANSI。
- rules-dir 缺省在交互与 scan 中应可使用随二进制编译的三套 bundled rules（通过 include_str! 或等价静态内容）；显式 --rules-dir 仍加载该目录并保留错误。不要依赖编译时源码绝对路径。添加已构建二进制在其他 cwd 工作的测试。

## 共享 cleanup 接口（C 实现；B 可临时 stub 独立验证）

```rust
pub fn prepare(home: &Path, rules: &[RuleSet], selected: &[Finding])
    -> Result<CleanupPlan, CleanupError>;
pub fn execute(plan: CleanupPlan) -> Result<CleanupResult, CleanupError>;

pub struct CleanupPlan {
    pub changes: Vec<PlannedChange>,
    pub skipped: Vec<SkippedItem>,
    pub backup_root: PathBuf,
    // C 定义私有快照字段；外部不能构造任意文件写入计划
}
pub struct PlannedChange {
    pub path: PathBuf,
    pub action: ChangeAction,
    pub item_count: usize,
    pub locators: Vec<String>,
}
pub enum ChangeAction { RemoveFile, EditJson }
pub struct SkippedItem { pub path: PathBuf, pub reason: String }
pub struct CleanupResult {
    pub removed_items: usize,
    pub changed_files: usize,
    pub backup_path: Option<PathBuf>,
    pub failures: Vec<SkippedItem>,
}
```

以上 public types derive Debug。所有文件副作用由 C cleanup 管理，不在 UI 写删除循环。result.failures 非空则 UI 返回失败退出码。核心不可依赖对话框或 TTY；它的 execute 接收的是已由调用方确认的 opaque plan。

## 清理执行器要求（C）

允许修改：src/cleanup/**、src/lib.rs（加 cleanup 模块）、tests/cleanup*.rs；Cargo 仅增加所需依赖且报告。不要改 CLI。必要的 scanner/rules 公共接口调整先报告，以兼容现有报告为前提。

1. prepare 对传入 selected 与当前 rules 扫描结果核对，不信任任意 Finding 的 path/locator；只接受仍存在且属性一致的条目，去重 physical target。对不支持/修改/冲突项生成 skipped。
2. 必须覆盖本轮所有实际规则：Muxy/Superset candidate hook action、Superset skill_marker 文件、Paseo manifest 列出的 unchanged 文件、Superset ledger 验证 unchanged MCP。
3. candidate hook 仅因用户显式选择传入才可计划，不自动扩展同名项目。confirmed modified 与 conflicting 不允许操作。unknown MCP 不能移除。
4. hook 精确移除最内层 action，同一 group 混有用户 hook 必须保留；多个 index 从同一快照统一规划避免位移误删。仅当本次操作清空 group 的 hooks 时删除该 group，event 数组空才删除 event；不得清理原本无关空字段。
5. MCP 移除指定 key，保留所有无关设置。JSON 编辑要保留无关字节/缩进优先，使用可验证的范围编辑或成熟 CST 库，不用 regex 删 JSON。绝不能无提示写掉 JSONC 或重复 key 的歧义输入；不支持则拒绝执行并给出原因。
6. skill_marker 只删除 SKILL.md；manifest 只删除选中的逐文件 unchanged 项。额外用户文件、修改文件保留。选中部分 skill 文件时保留剩余内容，不能目录递归删除。
7. 维护关联 manifest/ledger 中已经成功移除项，或在整个受管理文件集合已清空时移除专属 manifest；只能在确证 schema 和归属、已记录快照时操作。未选或未成功项保留；不要清掉还需要验证的证据。发现用户增添内容时保留目录，可仅 prune 本次清理后变空的专属 skill 子目录，不删共享 roots。
8. 备份默认 `home/.local/state/akcleaner/backups/<唯一id>`；计划只计算路径不创建。执行前私有目录权限 0700 / 文件0600，拒绝 symlink 各级组件，备份全部将修改/删除文件及路径映射（记录 json 不含原始命令）。不能在备份不完整时删除原件。保留原 mode；JSON 同目录临时文件原子替换。
9. plan 保存配置、目标、manifest、ledger 的原 bytes hash 与类型；执行前全部重新检查，任一依赖变动/软链接/越界则整计划阻止，确保 preview 后更改不会被覆盖。声明不能保证对不合作外部 writer 的跨文件事务；不要声称原子清理。
10. 执行失败以实际完成记录为准，返回部分失败与备份路径，不能报全部成功；文件被移除后再次扫描应不再命中。用户拒绝确认发生在 execute 前，无任何写入。
11. 没有正向证据的支持脚本、app 数据、worktree、runtime/cache 不参与删除；报告只说删除选中并列出的项目，不承诺整个产品已卸载。

测试至少：混合 hooks 多索引移除、混合 MCP、多产品同文件、modified/conflicting 跳过、marker skill 保留额外文件、manifest 全选和部分选择、ledger tracking 保留其他项、preflight 内容/证据/软链接更改阻止、备份失败零删除、取消零副作用由 B 测试、第二次扫描幂等、权限保留、无敏感日志。保持 read-only tests。

## CLI 实现授权与测试（B）

允许修改：src/cli.rs、src/interactive/**、src/rules 的 bundled loader 入口、src/lib.rs 对应 module exports、Cargo.toml/Cargo.lock、README.md、tests/interactive*.rs、tests/cli.rs。不修改 cleanup 或扫描归属逻辑；临时 cleanup stub 只为独立编译并必须标注，在最终集成前移除。

测试重点：无参数/clean routing、未知 product、bundle rules 外 cwd、非 TTY 拒绝 clean、scan JSON 无色且保持格式、无结果、不可删结果、空选择/确认 No/取消均不调用 execute、确认 Yes 才执行、失败结果非零。抽出小型 prompt 交互边界便于输入模拟，不要为测试引入大型 UI framework。

用本机可用 PTY 工具（Python pty 或现有工具）在合成 home 上验证真实键盘选择和确认，保存简洁终端 transcript 到测试或交付说明，禁止用真实配置做演示。

## 最终交付

两者各自跑 cargo fmt/check/clippy/test。完成后停止并汇报。负责人审阅后再授权 C 集成 B、跑组合测试、同步主目录；此时不要自动修改主目录。不得提交或推送。无需询问用户是否开始已明确授权的代码工作。
