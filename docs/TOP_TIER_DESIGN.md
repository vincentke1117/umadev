# UmaDev — 顶级项目总监 Agent 设计

UmaDev 是确定性 Rust 壳,驱动用户已登录的底座 CLI 跑一条受治理、可审计的 9 阶段交付流水线。要成为顶级项目总监 Agent,沿三条轴演进,**绝不牺牲护城河**(确定性、可审计、成本控制、fail-open、不持有模型端点)。

> 方法论取自对多个前沿 Agent 系统的调研,但本文档与落地代码**一律泛化为技术、不点名来源**。

## 不变量(任何升级不可破)
- fail-open by contract:出错=no-op,绝不阻塞底座
- 确定性控环:循环控制信号永远是确定性的(gap-count/退出码),LLM 只做 advisory
- 不持有模型端点:"底座生成"只走 host-driver 子进程,不新增 API key
- 依赖极轻、单写者(强耦合写)、phase 链作硬状态机、无来源署名

## A. 角色团队层(模拟真实开发团队)
- **总监** = 确定性 Rust runner(已是唯一编排者+控环者),保留。
- **写者** = 现有 phase 角色 persona,在**单条累积底座 session** 上串行写(强耦合写禁止并行——冲突的隐含决策会拼不到一起)。
- **评审 = 新增只读角色 critic**(PM/架构/QA/安全/UX critic),在 fork 出的隔离 session 上跑,**只读不写**,返回强 schema verdict `{accepts, blocking[], advisory[], evidence[]}`。
- **控环** = 确定性 gap-count + stall-counter(泛化现有验收循环);确定性地板(coverage/contract/governance)先跑当硬门,LLM critic 的 blocking 折进首轮打回。
- **团队规模按任务复杂度缩放**(Light/Bugfix=无 critic,Greenfield=全队)——成本主闸。
- **通信** = 结构化 verdict over 共享文件黑板(output/*.md)+ team-ledger 事件流;不用聊天(反幻觉)。
- **并行**只用于读/研究 fan-out(扩展现有 docs 并发),绝不并行写者。
- 步骤:① 形式化 `RoleCritic` trait + `RoleVerdict` schema(把现有 3 个 ad-hoc 裁判迁进去,纯重构)② 泛化验收循环成 `run_role_acceptance(phase, critics)`,先接 docs 阶段 ③ 复杂度缩放团队 ④ 主 session 单写者 + critic fork 隔离 ⑤ QA/安全 critic 接确定性地板。

## B. 自我进化记忆(学习系统)
现状:真闭合的 capture→sediment→retrieve→inject→efficacy 闭环,3 类记忆、衰减排序、失败修法规避、去重。升级:
- **P0-A 反思→生成新策略**:pitfall **复发后**,发一次便宜底座调用产出"诊断上次为何失败 + 一个不同的高层做法",存为 efficacy 上的 `next_strategy`,下次命中时 surface——替代现在干巴巴的"必须换方案"。只在复发时触发(首次失败仍走廉价模板),fail-open。
- **P0-B 双通道统一重排**:在 prompt 装配缝(coach)用**纯 Rust RRF(k=60)**合流指纹衰减通道 + BM25/RRF 知识通道,带每通道保底名额 + token 预算。**不进 knowledge crate**(守边界,不耦合)。
- **P1-C 可复用技能库**:**过质量门/契约**的产物才"毕业"入 `.umadev/skills/`(描述+recipe),按相似度 top-k 检索,按效用衰减退役;只收多步解出的。
- **P1-D 记忆整理/遗忘**:sediment 时对相似旧教训做 ADD/UPDATE/INVALIDATE/NOOP(标 invalid 不物删);久未命中的 sedimented 教训随衰减退出 top-k。
- **P2-E 检索增强**:假想答案扩展(对 BM25-first 价值最大)、可选 cross-encoder 重排、低-IDF token 掩码。全部门控/fail-open。

## C. 商业落地完整性
- **Tier-0 #1 brownfield 接入** `umadev adopt`:扫现有仓库→识别栈→逆向推导 API 表进 contract `ApiSpec`→把 BM25 索引器指向用户源码树→写边界契约 → 各阶段变**增量改动而非重写**。(最大产品缺口)
- **Tier-0 #2 运行时证据** `verify --runtime`:启动 dev server→用浏览器自动化 MCP 驱动主流程→截图+trace 进 proof-pack(从"跑了门禁"变"app 真在跑")。
- **Tier-0 #3 部署/交接**:部署适配器(识别平台)→ preview URL + 日志进 proof-pack。
- **Tier-1/2**:PR 评审清单交付物、分级信任模式(plan/guarded/auto + 自动通过率追踪)、可恢复/可 fork 的 run、原生 PR 模式、Pre-PR 安全扫描门。

## 实现顺序
Phase 2a(现在):记忆 P0-A + P0-B。 2b:角色 critic 层 ①②。 2c:brownfield adopt + 运行时证据。 2d:技能库 + 记忆整理。 之后:信任模式 / PR 模式 / 部署。
