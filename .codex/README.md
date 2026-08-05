# Codex custom agents

本目录将 `.opencode/agents/` 中的 subagent 配置迁移为 Codex 项目级 custom agent。

- `agents/senior-developer.toml`：复杂技术决策、故障定位和跨模块开发。
- `agents/quant-research.toml`：量化策略、数据口径和回测实现的咨询与审查。

Codex 启动时会从 `.codex/agents/` 读取项目级 agent。两个配置沿用原有模型角色，
并将 OpenCode 的 `variant: medium` 映射为 Codex 的
`model_reasoning_effort = "medium"`。
