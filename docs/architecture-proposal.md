# AKCleaner：Agent 集成清理架构提案

调研日期：2026-09-26。状态：讨论稿，尚未实现。

## 1. 结论

建议采用 **产品 rule set + 宿主 surface adapter + 有限清理原语**。

核心职责是：识别第三方应用留在 agent 环境中的集成，解释其归属，并生成精确、可恢复的移除计划。不要把一个产品实现成一个任意代码执行的 uninstaller，也不要把清理简化为产品名路径 glob。

最重要的区别：**「由某应用安装」与「内容属于某应用」是两条不同的关系**。例如 Superset 可以代装第三方 MCP/skills；清理 Superset 自带能力与撤销 Superset 管理的全部集成，应当是不同选择。

## 2. 调研范围与依据

阅读了下列公开仓库的当前快照、安装实现和官方文档，没有运行它们的安装器，也没有扫描用户真实 agent 配置。本表是已证实的行为示例，不是穷尽版本的安装清单。

| 产品 | 本次源码快照 |
| --- | --- |
| LoopX / `loopx-project/loopx` | `3e443ad7c285973c40be883293970be6c0c51e06` |
| Lody / `LodyAI/Lody` | `9c56cb5ed3c2318bb1f2b590abcaa4252db91b4f` |
| Superset / `superset-sh/superset` | `23401526cc6787def0b83ff8decef66ba288264a` |
| Paseo / `getpaseo/paseo` | `8cd989529e2d86bb6d1c8a775bf695fa5970c997` |
| Muxy / `muxy-app/muxy` | `ab008599954afe683a48410b98b8f483adb96b40` |
| Pencil / pen.dev | 官方文档，页面更新于 2026-09-24；未核实安装器源码 |

### LoopX

已证实：

- `workflow-skills --install` 向选定 Codex skill 根目录复制工作流；考虑 `CODEX_HOME/skills` 与 `~/.agents/skills`。
- `slash-commands` 为多个 host 写入 skill/command facade；生成内容带 `<!-- loopx-managed-slash-command:v1 ... -->` 标记。
- Cursor 集成可在 `mcp.json` 写入 `loopx`，使用 `.loopx-managed-mcp.json` sidecar 记录所写配置。
- 可选 OpenCode goal bridge 写入 `plugins/loopx-goal.js`、`loopx/goal-bridge-runtime.mjs`，并修改 `package.json` 依赖；官方卸载刻意保留可能共享的依赖。
- 存在项目级 skill 安装、旧 `loop-global-*` 别名与自定义 profile；不能只匹配当前全局 `loopx-*`。

架构含义：标记、安装回执、历史布局、项目范围、共享依赖都是真实需求。`.loopx/` 项目状态不是可随 skill 一并删除的垃圾。

来源：
- [安装指南](https://github.com/loopx-project/loopx/blob/3e443ad7c285973c40be883293970be6c0c51e06/docs/guides/installing-loopx.md)
- [command/MCP/bridge 安装实现](https://github.com/loopx-project/loopx/blob/3e443ad7c285973c40be883293970be6c0c51e06/loopx/slash_command_install.py)
- [managed marker](https://github.com/loopx-project/loopx/blob/3e443ad7c285973c40be883293970be6c0c51e06/loopx/slash_command_files.py)

### Lody

已证实：

- 官方 Skills 文档明确说明：Skills 功能只发现、展示和引用文件，不安装、编辑或同步 Skills。
- workspace MCP catalog 经会话解析，交给 ACP `newSession({ cwd, mcpServers, ... })`；不能据此推定向用户全局 MCP 配置写入了条目。
- 存在 managed agent runtime、ACP binary 的下载与暂存，以及 title-agent 的隔离 Codex home；这是 app-owned runtime/cache 类对象，不等同于全局注入残留。

架构含义：rule set 可以记录「仅运行时集成」和当前覆盖边界；零个可清理全局 artifact 是合法结果。不能因为 Lody 展示了某个 skill，就判定它归 Lody 所有。

来源：
- [Skills 文档](https://github.com/LodyAI/Lody/blob/9c56cb5ed3c2318bb1f2b590abcaa4252db91b4f/site-docs/content/docs/en/(features)/skills.mdx)
- [会话 MCP 解析](https://github.com/LodyAI/Lody/blob/9c56cb5ed3c2318bb1f2b590abcaa4252db91b4f/apps/cli/src/agent/session-mcp-resolver.ts)
- [ACP 会话启动](https://github.com/LodyAI/Lody/blob/9c56cb5ed3c2318bb1f2b590abcaa4252db91b4f/apps/cli/src/agent/agent-client.ts)
- [managed runtime](https://github.com/LodyAI/Lody/blob/9c56cb5ed3c2318bb1f2b590abcaa4252db91b4f/apps/cli/src/agent/managed-agent-runtime.ts)

### Superset

已证实：

- 向 `~/.claude/settings.json` 与 `~/.codex/hooks.json` 合并 hook，以 notify 脚本路径识别管理项。
- OpenCode 集成包含 `superset-notify.js` 插件。
- `~/.claude/skills/superset` 是 skills-directory plugin，带 `.superset-managed` sentinel。
- `~/.agents/skills` 中写入带 `<!-- superset-managed-skill v1 -->` 的 skills；`~/.agents/commands/superset` 中写入命令。
- 安装的 marketplace 插件也可被分发到这些位置，名字未必都以 `superset-` 开头。
- Claude MCP 位于 `~/.claude.json`；`~/.superset/plugins/mcp-ledger.json` 记录配置路径、键和规范化值哈希。用户修改过的条目被保留。
- Codex MCP 使用 `~/.codex/config.toml` 内的 managed marker block。
- 支持 secondary account/profile，skills 可能直接复制或通过共享链接获得。

架构含义：产品名匹配会漏报；仅 key 匹配会误报；ledger 必须参与识别，不能提前作为普通垃圾删掉。

来源：
- [hooks 与 OpenCode 插件](https://github.com/superset-sh/superset/blob/23401526cc6787def0b83ff8decef66ba288264a/packages/agent-setup/src/agent-wrappers-claude-codex-opencode.ts)
- [skills 分发](https://github.com/superset-sh/superset/blob/23401526cc6787def0b83ff8decef66ba288264a/packages/agent-setup/src/managed-skills.ts)
- [MCP ledger 与 TOML block](https://github.com/superset-sh/superset/blob/23401526cc6787def0b83ff8decef66ba288264a/packages/agent-setup/src/managed-mcp-servers.ts)

### Paseo

已证实：

- orchestration skills 可由 app 安装，或经 `npx skills add getpaseo/paseo` 安装；host 启动时刷新选中的 skills。
- app 内置同步实现复制到 agents、Claude、Codex 三个 skill 根目录，并写入 `.paseo-managed-files.json`，记录逐文件 SHA-256。
- terminal hooks 支持 Claude `settings.json`、Codex `hooks.json`，尊重 `CLAUDE_CONFIG_DIR`、`CODEX_HOME`。
- OpenCode 写入 `plugins/paseo-terminal-activity.js`，尊重 XDG 与 `OPENCODE_CONFIG_DIR`。
- shell hook 由 `PASEO_TERMINAL_ID` 守卫，调用 `${PASEO_HOOK_CLI:-paseo} hooks ...`。
- MCP 也有运行时注入方式，受 `daemon.mcp.injectIntoAgents` 控制；不代表必须存在全局 MCP 文件残留。

架构含义：同一产品有不同安装渠道，证据不同；文件存在与当前会话是否激活分开表达。清除复制文件不一定会取消 app 的安装意图。

来源：
- [Skills 文档](https://github.com/getpaseo/paseo/blob/8cd989529e2d86bb6d1c8a775bf695fa5970c997/public-docs/skills.md)
- [同步 manifest](https://github.com/getpaseo/paseo/blob/8cd989529e2d86bb6d1c8a775bf695fa5970c997/packages/server/src/server/orchestration-skills/internal/sync.ts)
- [hook installer](https://github.com/getpaseo/paseo/blob/8cd989529e2d86bb6d1c8a775bf695fa5970c997/packages/server/src/terminal/agent-hooks/agent-hook-installer.ts)
- [provider 定义](https://github.com/getpaseo/paseo/tree/8cd989529e2d86bb6d1c8a775bf695fa5970c997/packages/server/src/terminal/agent-hooks)
- [MCP 配置](https://github.com/getpaseo/paseo/blob/8cd989529e2d86bb6d1c8a775bf695fa5970c997/public-docs/mcp.md)

### Muxy

已证实：

- Claude `~/.claude/settings.json` 与 Codex `~/.codex/hooks.json` 中加入多个事件 hook。
- command 尾部有 `# muxy-notification-hook` 标记，指向暂存 hook 脚本。
- Pi 集成复制 `~/.pi/agent/extensions/muxy-notify.ts`；还处理旧 `settings.json` 的 `extensions` 注册项。
- hooks 包含暂存脚本和 bridge binary；`HookInstaller` 会检查、修复集成，禁用时卸载。

架构含义：需要同时表达配置引用与其 payload；旧注册方式可能与新自动发现方式并存。

来源：
- [Claude provider](https://github.com/muxy-app/muxy/blob/ab008599954afe683a48410b98b8f483adb96b40/Muxy/Services/Providers/ClaudeCodeProvider.swift)
- [Codex provider](https://github.com/muxy-app/muxy/blob/ab008599954afe683a48410b98b8f483adb96b40/Muxy/Services/Providers/CodexProvider.swift)
- [Pi provider](https://github.com/muxy-app/muxy/blob/ab008599954afe683a48410b98b8f483adb96b40/Muxy/Services/Providers/PiProvider.swift)
- [修复逻辑](https://github.com/muxy-app/muxy/blob/ab008599954afe683a48410b98b8f483adb96b40/Muxy/Services/AI/HookInstaller.swift)

### Pencil / pen.dev

官方文档已证实：

- 启动时配置支持的 MCP 集成，Settings → MCP 可以选择客户端。
- 包括 Claude Code、Codex、Gemini、OpenCode、Kiro、Claude Desktop 等；服务标识为 `pencil`。
- 外部客户端通过 stdio 与 MCP server 通信，server 再经 Unix socket / Windows named pipe 连接运行中的 app。

尚未核实：不同桌面版/IDE 扩展版的精确 command 路径、历史配置布局、是否写入 instruction 文件及具体标记。因此 `pencil` key 只能作为候选，正式删除规则还需 command/args、路径或上游安装证据。当前文档与旧版工具集合已经存在差异，不能把最新文档当作所有版本的安装规范。

来源：[安装文档](https://docs.pencil.dev/getting-started/installation)、[AI 集成文档](https://docs.pencil.dev/getting-started/ai-integration)。

## 3. 共性模型

### 3.1 两层，而非每种 skill/hook/MCP 一个清理器

**语义层**：`skill | hook | mcp | command | plugin | instruction | support-file`。

**物理层**：文件、软链接、结构化配置成员、带边界的文本块、manifest 管理的文件集合。

例如 hook 可以是 JSON 中一个 command，也可以是 OpenCode 自动加载的 JS 文件。skill 可以是目录、复制文件集合或共享目录的软链接。语义层用于展示和归属；物理层决定如何修改。

### 3.2 建议 Finding 字段

```text
Finding
  product_id / rule_id / variant_id
  kind
  host / profile / scope
  physical_locator          # 路径 + member selector / 文本区间 / 链接身份
  managed_by                # 谁安装/维护
  content_origin            # 内容来源，可以未知或第三方
  evidence[]                # marker、manifest、hash、command identity 等
  attribution               # confirmed / candidate / conflicting
  integrity                 # unchanged / modified / unknown
  references[]              # 已扫描范围内的引用
  regeneration              # known / possible / unknown；附来源
```

归属与修改状态是正交维度：文件确实由 Paseo 安装，但可能已被用户改写。不要用一个 0.93 置信分数掩盖这个差异。

### 3.3 Surface adapter 的职责

- 解析 home、XDG、已提供的 profile override、显式项目根目录。
- 枚举配置位置及 schema，把配置读成标准化 hook/MCP 等对象，同时保留原始 source locator。
- 将语义删除转换为最小格式保留编辑。
- 区分缺失、解析错误、无权限、未支持结构；这些状态不能都解释成「未发现」。

不能把启动 AKCleaner 的环境变量视为机器上所有 profile 的完整清单。默认根、当前 override、用户显式指定根要分别标记。清理报告必须声明扫描覆盖范围。

### 3.4 Rule set 的职责

- 产品 ID、别名、验证日期、上游版本/commit。
- 选择哪些 surfaces，定义 managed marker、路径/命令身份、manifest/ledger 布局。
- 声明安装渠道和历史布局变体；版本未知时依赖实际文件特征识别。
- 为每个 artifact 定义受限的移除动作及依赖关系。
- 记录再安装机制和已知关闭入口；没有证据时只提供说明，不猜配置键。

相同 Claude 配置布局由 adapter 维护一次。新增产品通常只新增 rule set；新配置格式/新操作语义仍需引擎扩展。这个边界比承诺「所有未来产品都只写 YAML」更可靠。

### 3.5 规则面向语义入口，适配器保留原始定位

规则优先选择 `claude.hooks.user`、`codex.hooks.user` 等 surface，不在每个产品中重复配置路径与 `hooks.*[*].hooks[*]` 的遍历逻辑。适配器提供标准化视图：

```text
HookAction
  event / matcher / type / command
  source_locator

McpEntry
  name / transport
  command / args / url
  source_locator
```

`source_locator` 指向扫描快照中的精确原始成员。标准化视图用于匹配，不能替代原始文档：未知字段、注释和布局仍由文档编辑层保留。规则命中后，adapter 将移除意图转换为最小编辑。

产品自有的 manifest/ledger 可以用受限结构选择器或小型内置 decoder 读取；不要给规则语言引入循环、函数或任意脚本执行。

## 4. 有限操作集合

初始只需要下列操作，采用封闭枚举，不开放任意 shell：

| 操作 | 语义 |
| --- | --- |
| `remove_member` | 删除 map 成员或数组内精确匹配项；hook 删除到最内层 action |
| `remove_block` | 删除唯一、完整、合法的 marker block；编辑后重新解析宿主文档 |
| `quarantine_file` | 将确认的独立文件转入本次 receipt 的隔离区 |
| `unlink` | 只移除指定软链接，不递归跟随目标 |
| `remove_manifest_files` | 展开成逐文件动作；哈希不匹配和额外文件保留 |
| `prune_empty_dir` | 仅移除确实为空且处于规则边界内的目录 |

不要提供默认递归删除 skill 目录的原语：`SKILL.md` 上的 marker 不代表用户后来放进去的所有文件都归该产品。缺少完整文件归属时，只移除可证实文件，报告剩余内容。

共享 JSON 中的清理例子：同一个 Stop matcher 下同时有 Muxy 与用户 hook，只删除 Muxy action。仅当其 hooks 数组被这次操作清空时，才按该 surface 的 schema 清理空 wrapper。

卸载支持脚本前，先删除选中的引用。如果扫描范围外可能仍有引用，或已扫描范围存在未选中的引用，默认保留 payload。共享 package.json 依赖不能凭「这个产品曾安装过」就删除。

## 5. 执行流程

```text
load rules → discover surfaces → parse once → match & attribute
           → Findings → select → Plan → preview
           → revalidate → journal & backup → apply → verify → Receipt
```

### 计划阶段

- Plan 保存规则版本、扫描范围、原文件摘要、精确目标、拟议差异和操作依赖。
- 同一物理文件的多个规则合并为一次 patch，处理冲突和重复命中。JSON 数组位置不作为跨快照的稳定身份。
- 先移除配置引用，再处理专属 payload，最后处理不再需要的 ledger/sentinel；部分清理应保留仍有意义的 ledger 内容。
- `candidate` 与 `modified` 项保留在报告中，默认不产生删除动作。

### 应用与恢复

- Apply 前重新校验文件内容、类型、软链接状态和边界；计划过期则重新生成。
- 文档写入使用同目录临时文件、权限保留、原子替换；JSONC/TOML 保留注释及无关布局。
- 哈希复检与原子替换不等于对任意外部 writer 的跨进程事务。若 app 持续刷新同一配置，要求先停其写入，再应用并复查。
- 每个文件记录执行前状态和执行后摘要，先持久化 journal 再变更；多文件失败返回明确的 partial 状态，支持恢复，不能声称跨文件原子性。
- Restore 仅在目标仍与本次执行后状态一致时自动恢复；存在后续用户修改则报告冲突。
- 备份包含敏感配置时使用私有权限；展示 diff 隐藏 token/env 值。

终态区分：`applied`、`partial`、`blocked`、`failed`。扫描没有发现可删项与扫描失败分开报告。幂等意味着清理后重跑不再修改文件，而不是吞掉所有错误。

## 6. Rule set 示例

以下是**建议的声明式语法**，不是已实现配置格式，也不是可直接执行的完整 Muxy 规则。

```yaml
schema: akcleaner.rules/v1
product:
  id: muxy
  name: Muxy

verified:
  repository: https://github.com/muxy-app/muxy
  commit: ab008599954afe683a48410b98b8f483adb96b40

rules:
  - id: notification-hooks
    kind: hook
    surfaces: [claude.hooks.user, codex.hooks.user]
    select:
      action_type: command
    ownership:
      all:
        - command_comment_equals: muxy-notification-hook
        - command_executable:
            path_within: "${product.hook_payload_root}"
    effect:
      type: remove_member
      target: matched_hook_action
      prune_empty: matched_hook_group

regeneration:
  behavior: repairs-managed-hooks
  guidance: 在 Muxy 中关闭相应 agent 集成，并退出 app 后清理。
```

`product.hook_payload_root` 必须由同一 ruleset 的已验证平台路径/定位声明定义；上述示例故意不填尚未核对的实际根目录。规则校验器拒绝未定义变量。命令识别只解析已支持的静态形态，不执行 shell 展开；未知 wrapper 降为 candidate。

Paseo skill 可以走 `remove_manifest_files`，读取 `.paseo-managed-files.json` 的 `files` 对象与 SHA-256。Superset MCP 可通过通用 ledger matcher 展开「文件路径 → entry key → canonical JSON hash」；所有 ledger 路径仍必须落在用户批准的 surface 范围内。

如果两个 manifest 形态尚不能由同一个简单解码器覆盖，可先用两个小型内置 decoder。不要为了让 YAML 无所不能，造出另一种编程语言。

## 7. Rust 项目组织建议

当前项目只有 `Cargo.toml` 和 Hello World，建议先保持单 crate：

```text
src/
  main.rs
  cli.rs
  rules/              # schema、校验、加载、归属判据
  discovery/          # scope/profile、发现、证据、物理去重
  hosts/              # Claude/Codex/OpenCode/shared skills surfaces
  cleanup/            # plan、动作、依赖、apply、receipt、restore
  documents/          # JSON/JSONC/TOML/文本最小编辑
rules/
  superset/
    rules.yaml
    evidence.md
    fixtures/
  paseo/
  muxy/
  loopx/
  pencil/
  lody/
```

先用具体模块与封闭类型；无需每个模块都包一层 trait，更不需要六个 ProductCleaner 实现。业务判定只产出计划，文件系统副作用集中在执行边界。

核心接口在概念上保持三步：

```text
scan(rules, scope) → ScanReport
plan(report, selection) → CleanupPlan
apply(plan) → CleanupReceipt
```

- `ScanReport` 包含发现、扫描覆盖范围，以及读取失败和未支持结构；不能只有成功命中项。
- `CleanupPlan` 包含确定的具体变更、原始文件摘要、预览 diff 和操作依赖。
- `CleanupReceipt` 记录实际执行结果、恢复材料位置和执行后摘要，支持恢复时检查后续修改。

CLI、后续 TUI 或桌面界面都调用这套核心；界面不自行实现归属判断或删除逻辑。暂不拆 Cargo workspace，等出现独立复用或发布需求再拆 crate。

候选依赖：`clap`、`serde`、`serde_json`、`toml_edit`、`sha2`；YAML 与 JSONC 的解析/保格式编辑库在实现前选型验证。不要把 `serde_json` 整文件重序列化当作 JSONC 编辑器。

建议命令界面：

```text
akcleaner scan
akcleaner scan --product superset --project ~/Developer/example
akcleaner plan --product superset --output plan.json
akcleaner apply plan.json
akcleaner restore <receipt-id>
akcleaner rules check
```

不需要数据库或常驻 daemon；初期规则跟二进制发布，可显式加载本地 ruleset。动态远程规则更新以后再做。

## 8. 规则维护与验收

每个产品目录是一个维护单元：规则、固定 commit 证据、before/after fixtures 一起 review。新增路径不是充分依据，必须新增归属判据和相应反例。

共享 fixture runner 的关键用例：

1. 同一 hook group 中混有两个产品和用户 action，只删除目标 action。
2. 名字相同但命令不同的 MCP、名字相同的用户 skill 不被删除。
3. manifest 文件被修改、目录包含用户额外文件时保留这些内容。
4. global、project、自定义 profile 分离；根目录别名不重复修改。
5. skill symlink、断链、父目录 symlink、越界 ledger 路径不会导致跟随删除。
6. JSONC 注释、TOML 无关表、CRLF、重复键/歧义 marker 有明确行为。
7. plan 后配置变更时拒绝旧计划；部分失败可读回执行记录。
8. apply 后再次 scan/plan 无额外变更；restore 遇到后续修改不覆盖。
9. 两份产品规则命中同一物理成员时报告冲突或去重，不能重复执行。
10. app 再安装导致残留回归时，报告重新出现，而非静默循环删除。

## 9. 推荐实现顺序与待确认边界

1. **先做只读扫描**：Claude/Codex/shared skills，Muxy + Paseo + Superset 用来验证三个最重要的真实模型：嵌套 hook、逐文件 manifest、MCP ledger。
2. **再做计划、精确编辑与恢复**：先覆盖上述 fixtures，再开放 apply。
3. **扩展 LoopX/OpenCode/项目范围**：加入 bridge、command、旧布局、JSONC。
4. **Pencil 补实机安装证据**：桌面版与 IDE 扩展分别取证；Lody 保留明确的 runtime-only/未覆盖说明，确认有持久注入再增加删除规则。

建议首版产品范围：**清除选定产品已持久化的 agent 集成**。应用本体、会话数据库、项目工作树、凭据和通用 runtime/cache 不属于这一模式。

「保留 app，但永久关闭全部 agent 集成」需要控制 app 的安装意图，有时涉及内部数据库/设置 API，维护成本显著不同。建议作为后续单独能力，而不是默认清理时隐式修改。

### 扫描报告示例

以下为建议的展示格式，数字是示例，不代表当前机器的扫描结果：

```text
Superset

Confirmed
  Claude Code   3 hook actions
  Codex         2 MCP entries
  Shared skills 4 managed skills

Needs review
  Claude Code   1 MCP entry changed since installation

Coverage
  Default user profiles
  No project directories scanned
```

局部扫描不能显示成整台机器已清理完成。用户必须能看出扫描了哪些 profile、哪些项目，以及哪些入口因为错误或能力限制未被检查。

## 10. 提案摘要

**规则描述归属，引擎负责精确移除，执行记录负责恢复。**

- 一个产品一个规则包：规则、证据和 fixtures 一起维护。
- 一个宿主维护一套配置知识，产品规则复用语义入口。
- 清理单位细化到配置成员、文件、软链接或受管理文件集合。
- 归属与修改状态分开表达，默认保留已修改内容并说明原因。
- 安装管理者与内容来源分开记录，覆盖应用代装第三方集成的情况。
- 首版使用单 crate、CLI 和随版本发布的静态规则。

待确认的产品决策：首版是否限定为清理已持久化集成，并把「保留 app 且永久禁止再次注入」作为后续独立能力。本文推荐这一边界；记录提案不代表已经确认实施。
