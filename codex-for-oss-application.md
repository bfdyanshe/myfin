# Codex for Open Source 申请表单（草稿）

官方申请地址：<https://openai.com/form/codex-for-oss/>

## 推荐项目

| 项目 | 结论 | 公开信号 |
| --- | --- | --- |
| [`bfdyanshe/myfin`](https://github.com/bfdyanshe/myfin) | 当前最适合申请，但属于早期项目，竞争力有限 | 公开、非 fork、Rust；2026-08-05 最近提交；0 stars、0 forks；无 Release；账号所有者为主要维护者 |

截至 2026-08-06，没有发现同时具备较强采用量和持续维护证据的仓库。其余原创公开仓库中，`openaiapilatency` 有 3 stars 但已长期缺少近期维护；`bfdyanshe.github.io` 和 `k2` 各有 1 star；`SICP` 与 `verification-algorithm` 均为 0 stars 且较早停止更新。其余公开仓库主要是 fork，不适合作为“本人主要维护的项目”申报。

## 表单填写内容

### First name *

`[填写名字]`

### Last name *

`[填写姓氏]`

### Email *

`[填写与你的 ChatGPT 账户关联的邮箱]`

### GitHub username *

`bfdyanshe`

提交前请确认 GitHub 个人资料为公开可见。

### GitHub repository URL *

`https://github.com/bfdyanshe/myfin`

提交前请确认仓库为公开可见，并补充仓库描述与许可证信息。

### Describe your role: are you a primary or core maintainer? *

选择：`Primary maintainer`

### Why does this repository qualify? *

以下内容控制在 500 字符以内；提交前可按最新 GitHub 数据更新数字：

> `myfin` is an actively maintained Rust workspace for reproducible, auditable A-share quantitative screening and backtesting. It combines free market and financial data sources with source failover, data-quality gates, point-in-time inputs, and backtest validation. I am the primary maintainer, responsible for architecture, implementation, maintenance, and release preparation. The project is new (0 stars/forks as of Aug 6, 2026), but is designed for transparent reuse in open financial-data tooling.

### I’m interested in...

建议勾选：

- `Codex Security`
- `API credits for my project`

### OpenAI Organization ID *

`[从 https://platform.openai.com/organization/settings/general 获取并填写]`

### How will you use API credits for your project? *

以下内容控制在 500 字符以内：

> `myfin` will use API credits for maintainer automation: reviewing pull requests, checking Rust and Python changes, validating data-source adapters and schema contracts, running data-quality and backtest regression checks, and preparing release notes and documentation. The goal is to reduce the time required to keep a multi-crate Rust workspace and its Python data-source integrations reliable while preserving reproducible, auditable research workflows.

### Anything else we should know?

以下内容控制在 500 字符以内：

> `myfin` is intentionally built around free data sources and reproducibility. Its maintainability depends on catching upstream API changes, data gaps, look-ahead bias, and schema drift early. I would use Codex in a review-first workflow, with tests and quality gates remaining authoritative. I am also willing to document maintainer workflows and share improvements that may benefit other open-source quantitative-finance projects.

## 提交前检查

- [ ] 填写姓名、ChatGPT 账户邮箱和 OpenAI Organization ID。
- [ ] GitHub 个人资料和 `myfin` 仓库均为公开可见。
- [ ] 仓库首页补充准确的项目描述、许可证和用途说明；当前 API 元数据显示 GitHub 未识别到许可证。
- [ ] 不填写无法证明的月下载量、用户数、社区采用量或生态影响力。
- [ ] 检查三段英文回答均未超过 500 字符，并确认表单条款后再提交。
