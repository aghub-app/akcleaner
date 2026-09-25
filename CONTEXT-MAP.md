# AKCleaner context map

当前为架构提案，尚未实现。

## Contexts

- **集成识别** — 在指定用户、profile、项目范围内发现集成残留，结合产品规则判断归属，输出带证据的发现。
- **清理执行** — 将选中的发现编译为可预览的文件变更，执行、验证并记录可恢复的操作。

## Relationships

- **集成识别 → 清理执行**：传递发现、归属证据和原始定位；发现本身不执行删除。
- 产品规则提供产品身份和归属判据；宿主适配器提供 agent 配置位置及结构；执行器提供统一文件操作语义。

## Language

- **Product**：提供集成的应用，如 Superset、Paseo。与执行 agent 的宿主区分。
- **Host**：消费集成的 agent 或客户端，如 Claude Code、Codex、OpenCode。
- **Surface**：宿主暴露的具体集成入口，如 Claude hooks、Codex MCP、共享 skills 目录。
- **Artifact**：一个可独立定位的集成单元，可以是文件、软链接、配置成员或文本块。
- **Finding**：实际发现的 Artifact，包含产品关联、证据、修改状态、引用关系与物理定位。
- **Rule set**：一个产品的声明式识别和清理规则，附源码证据与测试夹具。
- **Plan**：针对扫描快照生成的具体变更集合；不是再次扫描并自由执行的脚本。
- **Receipt**：AKCleaner 自己的执行记录和恢复材料；与上游产品的安装 manifest/ledger 区分。

架构及调研依据：[docs/architecture-proposal.md](docs/architecture-proposal.md)。
