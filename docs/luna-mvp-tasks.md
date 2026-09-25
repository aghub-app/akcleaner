# Luna 第一轮委派：只读扫描 MVP

本轮负责人负责审阅；三个 `codex/gpt-6-luna` 分别在隔离 worktree 实现。先阅读 `CONTEXT-MAP.md` 和 `docs/architecture-proposal.md`。本文件将讨论稿收敛为第一轮可独立开发的接口约定，不代表已经实现。

## 共同要求

- 本轮实现可运行的只读扫描，不实现 apply、删除、隔离、禁用集成或 restore。
- 不读写真实用户的 agent 配置、不运行产品安装器、不启动待清理产品。所有验证使用临时目录和合成 fixtures。
- 不提交、不推送，不修改其他 worktree；只修改自己的授权文件。禁止启动额外 agent。
- 不修改现有架构文档或本任务书。需要改变共享接口时，先在交付说明中报告，不自行另立 schema。
- Rust edition 2024、单 crate。使用明确 struct/enum、可复用错误用 thiserror、应用边界用 anyhow。避免无意义 trait、泛型框架和静默吞错。
- 不把解析失败当未发现；不根据产品名字子串认定归属；不跟随目录软链接扫描。
- 结果不得包含完整命令、token、env、headers、原文件内容，只报告必要定位和证据摘要。
- 不修改 git config、不绕过 hooks，不做提交。完成后输出：改动文件、通过的检查、限制和待集成事项。

## 第一轮共享协议

### Rule set（JSON，文件名 rules.json）

第一轮用 JSON，避免在尚未确定规则模型时额外引入 YAML。后续可增加 YAML 前端。

```json
{
  "schema_version": 1,
  "product": { "id": "muxy", "name": "Muxy" },
  "verified": { "repository": "https://github.com/muxy-app/muxy", "commit": "固定完整 SHA" },
  "rules": [
    {
      "id": "notification-hooks",
      "surfaces": ["claude.hooks.user", "codex.hooks.user"],
      "evidence": { "type": "command_marker", "marker": "muxy-notification-hook" }
    }
  ]
}
```

允许的 surface 恰为：`claude.hooks.user`、`codex.hooks.user`、`claude.mcp.user`、`shared.skills.user`、`claude.skills.user`、`codex.skills.user`。

允许的 evidence tagged union：

1. `command_marker { marker }`：仅 hook；命令含 marker 是 candidate，不宣称 confirmed。确切 parser 归属增强留后续。
2. `skill_marker { marker }`：仅 skill；SKILL.md 中确切受管理标记可以确认该文档的归属，不代表整个目录。
3. `skill_manifest { filename }`：仅 skill；本轮只支持 Paseo 的 version=1、files: {relative_path: sha256}。确认每个列出的普通文件，逐个判断是否修改；manifest 不能授权越界访问。
4. `mcp_ledger { path }`：仅 Claude MCP；本轮只支持 Superset 的 version=1、files: {absolute_config_path: {entry_name: sha256}}。`path` 为 home 下相对路径。仅对当前扫描 home 的 `.claude.json` 生效，不跟随 ledger 任意路径。

未支持 schema_version、未知字段/证据类型、重复 product/rule ID、空 marker、非法 product ID、含绝对路径或 `..` 的 manifest filename/ledger path、surface/evidence 不兼容，都应有明确校验错误。

fixture 是合成数据，不包含真实用户路径/秘密；ledger 依赖绝对 home 时由测试在临时目录生成，不能硬编码一台机器的 home。

### Host adapter 接口（Agent B 负责）

模块文件为 `src/hosts/mod.rs`，核心实现只调用：

```rust
pub fn discover(home: &std::path::Path, surfaces: &[String]) -> HostInventory;

pub struct HostInventory {
    pub artifacts: Vec<HostArtifact>,
    pub diagnostics: Vec<HostDiagnostic>,
}

pub struct HostArtifact {
    pub surface: String,
    pub path: std::path::PathBuf,
    pub locator: String,
    pub value: serde_json::Value,
}

pub struct HostDiagnostic {
    pub surface: String,
    pub path: std::path::PathBuf,
    pub code: String,
    pub message: String,
}
```

所有 struct derive Debug；不强制 Serialize，核心自己选择公开输出字段。

定位和 value 约定：

- hook：path 是配置文件，locator 是 RFC 6901 JSON Pointer，如 `/hooks/Stop/0/hooks/1`；value 是原始 action 对象。只读解析，保留 sibling，不返回全局配置。
- MCP：path 是 `home/.claude.json`，locator 是 `/mcpServers/<escaped-name>`；value 是对应 server 对象。
- skill：path 是实际 `SKILL.md`，locator 为空字符串；value 为 `{ "skill_dir": "绝对路径", "content": "SKILL.md 内容" }`。仅读取 immediate child skill，不递归遍历其文件。
- 内部 value 可以包含敏感内容用于判定，但不能被 CLI 直接序列化到报告。

路径映射：Claude hooks → `.claude/settings.json`；Codex hooks → `.codex/hooks.json`；Claude MCP → `.claude.json`；skills → `.agents/skills`、`.claude/skills`、`.codex/skills`。

本轮仅默认用户 profile（或 `--home` 指定测试 home）；项目与环境 override 暂不支持，报告必须声明。

## Agent A：规则证据和合成 fixtures

允许修改：`rules/**`、`docs/research/mvp-rule-coverage.md`。不得修改 Cargo 或 src。

交付：

1. Muxy、Paseo、Superset 各一个 `rules/<id>/rules.json`，符合上述协议。
2. 每个产品的 `evidence.md`，引用架构文档固定 commit 的文件/行及解释。不要把仓库里的开发者自用配置误当成产品安装行为。
3. fixtures 包含 before/expected（只读扫描预期，不是删除后的文件）、说明和反例。目录组织自行选择，但必须 README 说明如何读和后续接入测试。
4. Muxy 的 command marker 本轮只能 candidate；Superset skill marker 支持确认 SKILL.md；Paseo manifest、Superset MCP ledger 用独立最小样本。
5. 覆盖：同一 hook group 两家产品与用户 hook、同名但无 marker 的 skill、modified manifest file、manifest `../` 路径、同名 MCP 但 ledger 不匹配、JSON pointer 特殊字符 key。
6. 明确列出未覆盖项：其他产品、运行时注入、再安装设置、Codex TOML、项目/profile 等。不能把没有实现的入口标为支持。

源码已在本机临时研究目录：`/var/folders/9d/2_f44q0j3dx6zqbc3lt5y6940000gn/T/opencode/akcleaner-{muxy,paseo,superset}`。只读这些源码即可，不重新安装产品。可以直接引用固定 GitHub 链接。

验证：用现有解释器/工具校验所有 JSON 语法；逐项核对 evidence 与 fixture 预期。无需为此安装前端依赖。

## Agent B：只读 Host adapter

允许修改：`src/hosts/**`；为独立验证可以在自己的 worktree 临时修改 Cargo.toml、Cargo.lock、src/lib.rs，但交付必须说明这些为集成脚手架，由 Agent C 的版本作为最终基准。不得修改 src/main.rs、规则或核心模块。

依赖限定：serde_json、thiserror；测试可使用 tempfile。

实现上述完整 discover 接口，支持六个 surfaces，遍历和结果排序确定性；缺失配置正常为空；未知 surface、权限/读取失败、无效 JSON、错误顶层 schema 分别报告诊断。不要支持 JSONC by stripping comments；本轮这些 surface 只接受 JSON。

明确要求：

- 使用 symlink_metadata 检查 home 下涉及路径的各级组件；发现 symlink 则报告 skipped_symlink，不跟随。测试 home 自身由调用者提供，但其内 `.claude` 等根不得被跟随。对 macOS 临时根的系统路径别名不盲目拒绝。
- hook 逐 action 枚举；type 不是 command 可以忽略；type=command 但 command 不是 string 应报告 schema 诊断。无关设置不视为错误。
- MCP 只枚举顶层 mcpServers；项目嵌套明确未覆盖。
- skill 只读 immediate child 的 SKILL.md，遇到 symlink、非普通文件或非法 UTF-8 诊断；不能递归任意用户目录。
- 设置合理配置/skill 文件大小上限（常量即可），超限诊断；错误 message 不带原始内容。
- 用 serde_json 标准 pointer escaping 保证含 `/`、`~` 的键可定位。
- 不实现写操作或归属判断，不输出日志泄漏原文。

测试：mixed hooks、缺失文件、无效 JSON/schema、特殊 key、多个 skills、目录/文件软链接、超限文件；为测试创建临时目录，不触碰真实 HOME。

检查：cargo fmt -- --check；cargo check --all-targets；cargo clippy --all-targets --all-features -- -D warnings；cargo test。

## Agent C：规则模型、扫描核心和 CLI

允许修改：Cargo.toml、Cargo.lock、src/main.rs、src/lib.rs、src/cli.rs、src/rules/**、src/discovery/**、tests/**、README.md。不得修改 docs、产品 rules 或 Agent B 的 hosts。

依赖建议：clap derive、serde derive、serde_json、thiserror、anyhow、sha2；按适用技能用 garde 进行 rule DTO 校验；测试 tempfile。不要引入数据库、异步运行时、网络、GUI。

实现：

1. schema_version=1 的 strict rule loader、类型、语义校验。规则目录按产品子目录的 rules.json 加载；全部 rules check 拒绝重复 product ID。
2. `akcleaner rules check --rules-dir <path>`。
3. `akcleaner scan --rules-dir <path> --home <path> [--product <id>] [--json]`。home 缺省为当前用户 home；测试全部显式传 home。rules-dir 缺省为 cwd/rules，并在不存在时给明确错误，不能报告零残留成功。
4. ScanReport 包含 findings、diagnostics、coverage；finding 包含 product_id/rule_id、surface/path/locator、attribution、integrity、简短 evidence 摘要。不要输出内部 value 或 raw command。
5. Attribution 枚举 confirmed/candidate/conflicting；Integrity 枚举 unchanged/modified/unknown。没有历史哈希时 integrity=unknown，不凭 marker 宣称 unchanged。
6. manifest 按逐文件 SHA-256 校验。只接受 64 位十六进制哈希；拒绝绝对路径、父级逃逸、任何被访问路径组件 symlink、超限输入；合法但不存在的列出文件报告诊断。额外用户文件不算产品文件。
7. Superset ledger 的 canonical JSON hash = 递归对象 key 排序、数组顺序不变、紧凑 JSON UTF-8 SHA-256。未追踪 MCP 不认领；tracked hash 不同则 confirmed+modified，并解释仅表明历史管理关联。ledger 内容不能授权读取其他 home/profile 文件。
8. command_marker 是 candidate+unknown；skill_marker 是 confirmed+unknown（归属范围仅 SKILL.md）。manifest hash 一致为 confirmed+unchanged，不一致为 confirmed+modified。
9. 使用 B 的 discover 接口。B 尚未交付时，可以在自己 worktree 创建最小临时 hosts stub 或测试 shim，但必须标明临时，不能交付为已完整集成或在 stub 下宣称扫描完成。最后交付时需列明待替换。
10. 规则加载错误非零退出；扫描诊断意味着不完整，输出报告但非零退出；正常零 findings 退出 0。排序、去重稳定。报告说明只覆盖默认用户 roots，未覆盖 project/profile override。

测试：严格 schema、重复 ID、surface/evidence 不兼容、路径逃逸/软链接/非法哈希、manifest modified、canonical JSON key order、ledger 越界、敏感值不出现在序列化报告、unknown product、缺失规则目录、可重复确定性扫描。先测试纯归属逻辑，集成 hosts 后再补 CLI 测试。

README 包含已实现功能、明确限制、测试命令、合成 home 的使用例子。不得写 apply 已可用。

检查同 B。不能为通过测试删除有意义的测试或扩大容忍范围。发现契约矛盾就报告。

## 集成与审阅

负责人先审阅三份变更及证据，再将问题发回对应 Luna。Agent C 在后续明确指令下负责集成 A/B；负责人不代写实现。不自动合并分支或创建新提交。终态报告必须区分独立模块测试通过与最终组合测试通过。
