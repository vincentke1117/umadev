# UmaDev 企业级成熟度审计（2026-07-14）

> 审计对象：`9433f09e37d3` 之上的本地工作树（包含尚未发布的修复）；审计文字更新于 2026-07-17，当前工作树最终复验状态见 §7。
> 本文是一个时间点快照，不替代规范，也不代表 npm/GitHub 已发布版本已经包含这些改动。
> 规范真相源始终是 [`UMADEV_HOST_SPEC_V1`](../spec/UMADEV_HOST_SPEC_V1.md)。

## 结论先行

UmaDev 的产品方向和核心编排架构是成立的：它已经能以一个协调者席位，把八个专业席位、
一个可持久化计划、单写者执行、并行只读评审、机械验收、交付证据和项目记忆组合成一条
真实可运行的研发链。它不是简单地给 Claude Code、Codex、OpenCode、Grok Build 或 Kimi Code 套一段提示词。

但“核心理念成立”不等于“已经完全达到企业级成熟产品”。截至本次审计，准确结论是：

- **团队型 Agent 的主干已实现**，架构选择也比让多个 Agent 自由聊天更可控；
- **可靠性、权限、升级、自进化和交互中的多处闭环缺口已在本地修复**；
- **维护性、跨终端实机认证、治理规则自身的剩余债务、原生签名和发布外部配置仍未全部
  收口**；严格 rustdoc 与 all-features 测试密闭性已收口并进入自动化验证；
- 因而当前应定位为“功能完整、进入系统性硬化期”，不能宣称“所有平台、所有终端零缺陷”
  或“企业级已经彻底完成”。

## 1. 产品理念是否真的落地

| 理念 | 当前实现证据 | 审计判断 |
|---|---|---|
| 一个协调者管理真实研发流程 | [`router.rs`](../crates/umadev-agent/src/router.rs)、[`plan_state.rs`](../crates/umadev-agent/src/plan_state.rs)、[`director_loop.rs`](../crates/umadev-agent/src/director_loop.rs) 持有类型化路由、计划 DAG、步骤状态和循环控制 | 已落地 |
| 八个专业席位 | [`critics.rs`](../crates/umadev-agent/src/critics.rs) 定义产品、架构、UI/UX、前端、后端、QA、安全、DevOps 八席 | 已落地 |
| 像团队一样分工，而不是八个名字 | 做事席位在主会话串行执行；评审席位使用隔离、只读 fork；`RoleVerdict` 是类型化结果 | 已落地，且模型比自由群聊更稳健 |
| 用户能看见决策和进度 | `IntentDecided`、`PlanPosted`、`PlanStepStatus`、`CriticVerdict` 进入 TUI 事件面 | 已落地 |
| 完成必须有客观证据 | 覆盖率、契约、真实 build/test/runtime proof 和质量门控制完成状态；QuickEdit/Fast Debug 的代码写入要求最后写入后的定向验证；Director 的 Blocked/未完成/dirty/budget 残留统一以 `Failed` 结算；语义评审和完成自述不能覆盖机械失败 | 已落地 |
| 五底座一致 | [`BACKEND_IDS`](../crates/umadev-host/src/lib.rs) 锁定 `claude-code` / `codex` / `opencode` / `grok-build` / `kimi-code` 五个驱动；`RuntimeKind` 仅保留为兼容 wire tag | 支持面与身份边界已锁定；协议差异仍需持续兼容测试 |
| 项目越用越懂 | 项目事实、事故、反思、可复用规则和检索反馈均有持久化通道 | 已落地，但本轮修正了错误归因和“看起来学了、实际没验证”的问题 |

这里需要保持一句诚实表述：八个席位共享用户选择的底座大脑，它们是**受协调器约束的专业
工作视角和会话分支**，不是八个独立模型服务。对本地 CLI 产品而言，这恰好保留了上下文、
成本和单写者一致性；营销与文档也应一直使用“模拟真实开发团队工作”，不要暗示八个独立
模型在无约束自治。

## 2. 架构与分层

工作区当前有 12 个 Rust crate，职责边界总体合理：

- `umadev-spec`：规范数据真相源；
- `umadev-governance`：规则、审计、合规和策略地板；
- `umadev-contract`：OpenAPI 3.1 和前后端契约；
- `umadev-knowledge`：BM25、本地向量和混合召回；
- `umadev-runtime`：运行时抽象，不持有模型端点；
- `umadev-host`：恰好五个底座的子进程/会话协议适配；
- `umadev-process`：Windows Job Object 等跨平台进程树生命周期原语；
- `umadev-agent`：路由、计划、团队、验收、记忆和交付；
- `umadev-tui`：终端状态与渲染；
- `umadev-i18n`：三语用户文案；
- `umadev-state`：跨平台安全持久化与用户可控的叶子记忆策略；
- `umadev`：CLI、MCP、安装、升级和组合根。

内部依赖方向清晰，未发现 crate 级循环；`spec` 在底层，`runtime` 与 `host` 分离，二进制
在组合根。这些符合企业级可演进架构的基本要求。

真正的维护性风险在**crate 内部巨型模块**。当前约 32.8 万行 Rust（含测试），最大的生产文件包括：

| 文件 | 约行数 | 风险 |
|---|---:|---|
| `umadev-tui/src/app.rs` | 18,112 | 生产状态与交互仍宽，但约 9,600 行测试已迁出 |
| `umadev-tui/src/lib.rs` | 11,481 | 会话编排与事件循环仍是较宽的组合根 |
| `umadev-tui/src/ui.rs` | 11,790 | 多视图渲染集中，跨终端回归面大 |
| `umadev-agent/src/lessons.rs` | 11,520 | 多种记忆类型、迁移、归因和展示仍有耦合 |
| `umadev-agent/src/runner.rs` | 9,914 | 新旧执行路径仍有历史重量 |
| `umadev-governance/src/rules.rs` | 8,957 | 生产规则注册表；测试、collect-all、文件安全、深嵌套与调试残留分析已迁出 |
| `umadev-agent/src/director_loop.rs` | 6,741 | 主状态机已分离恢复、契约反馈和测试模块 |

workspace 架构守护把四个历史热点锁为精确 LOC ratchet：任何增长失败，缩短后必须同步下调
基线。首轮真实拆分已经完成：App 测试、规则测试、完整扫描、Director 恢复/契约测试等均已迁到
边界模块；四个主文件相较审计初始值都显著下降。守护还解析普通、build、dev 和 target 依赖，
执行 crate 边 allowlist、生产图无环及 foundation 不反向依赖 agent/TUI/二进制。剩余较宽的 App、
TUI 事件循环、UI 和 lessons 应继续按状态/协议/视图/存储边界拆分，但已不再是“只有 ratchet、
没有拆分”的状态。

## 3. 自进化：`/pitfalls` 与 `/lessons`

两者必须是两个互不重复的视图：

- **`/pitfalls` 是事件账本**：同一归一化签名有多少次独立发生、首次/最近时间、证据、
  修复尝试及验证状态；
- **`/lessons` 是规则库**：从复发事故或机械验证结果沉淀出的可复用规则，显示待验证、
  已验证或需要修订，不再重复展示原始事故。

正确闭环是：

```text
独立事故 → 同签名再次独立发生 → 候选规则 → 精确修复尝试
        → 同一问题的机械验证通过 → 已验证 lesson
        → 同一问题仍失败         → 修复失败，继续反思
        → 证据不充分/失败不同     → unknown，不伪造结论
```

“测试了两次”的有效定义是**两个独立执行 episode 中出现同一隐私安全指纹**。一次命令的
stderr 重复打印两行只算一次；打开两次 `/lessons` 也不产生证据。达到两次后，产品至少应
显示一条“待验证规则”或解释为什么只形成候选，不能继续空白。

旧版把宽泛的 `failed` 聚合成“已踩 226 次”，这些历史行没有足够时间和错误身份，不能被
伪造成 226 条带日期的新事件。本轮策略是保留为**隔离的 legacy audit 计数**，不参与行为、
信誉或规则晋升；只有新版本收集到的精确证据进入闭环。

这类“自进化”属于带验证的 episodic/semantic memory 与 reflection，不是对底座模型权重做
训练。成熟标准应是可追溯、可撤销、保守归因，而不是数字增长得快。

本轮闭合的是**精确 pitfall → 修复尝试 → 同错验证**这条链。检索到的通用知识块现在带有
内容绑定的稳定 memory ID；只有最终指令中实际存在该精确 marker、且底座接受了该指令，才
提交一次性 sent receipt，随后由机械 Pass/Fail/Unknown 结算，崩溃后的 intent 也可幂等回放。
普通非 pitfall lesson 的被动召回仍保持只读，不把宽泛的后续成败反向归因。也就是说，知识块
排序和精确 pitfall 已形成可审计反馈，所有记忆类型并没有被不加区分地宣称为“彻底自进化”。

跨项目记忆还需要更严格的隐私边界。本轮把 `~/.umadev/learned` 收窄为：只有已经独立复发
或被同一验证器机械验证的已知开发错误家族可以晋升，而且全局文件只保留白名单中的
`category/family`；私有包名、符号、路径、证据、需求和签名判别段全部留在项目内。非
DevError lesson 在有类型化脱敏协议前不跨项目晋升；旧版自动生成的非安全全局文件会被隔离。
跨项目投影不信任可编辑 raw JSONL 中的 domain/root-cause/fix，而是从分类器白名单重建；
自动文件只能用完整 YAML front matter 中规范的唯一安全字段证明身份；重复字段、引号/注释等
未实现的等价 YAML、缩进字段、读取失败和正文伪造都按不可信处理。用户的 `$HOME` 可合法指向
挂载或集中式家目录，但 UmaDev 管理的 `.umadev/learned` 祖先与根目录必须是真目录；隔离采用
“原子移出到非 Markdown staging、二次复核、再原子提交”，不删除可能由并发写者重建的新路径。
raw domain 只能是有界的 ASCII 单路径段，避免沉淀时路径穿越。知识索引对已审核 learned 文件
只读一次，同一份字节同时用于签名和分块；schema 升级会同步清理旧 BM25/向量缓存，任一文件
因 Windows 占用或 ACL 删除失败时不推进版本标记，下次启动继续重试，避免旧私有元数据滞留。

## 4. 权限与底座控制

UmaDev 的三档应该始终是显式、可见、可恢复的产品语义：

- `Plan`：只读研究和规划；
- `Guarded`：默认，具备文件、进程、网络和本地端口等完整开发环境，但保留危险动作审批；
- `Auto`：环境能力与 Guarded 相同，在用户明确选择后预授权普通底座审批，同时保留 UmaDev
  自身不可逆动作地板。

模型语义路由不能替代权限判定。模型结果只有精确合法的 `authorization: "mutating"` 才能
授权写入；缺失、空白或非法值一律 fail-closed 到只读 Explain，不启动写者或团队。独立的
确定性可用性兜底只从当前用户文本识别无歧义、窄范围的显式请求并留在常驻路径，绝不继承
非法模型字段的权限。`Plan` 还是独立的硬上限：即使模型把意图判为 Build/Debug 或返回
mutating，执行面也必须保持只读。

本轮把权限档持久化到工作流状态，并贯穿新运行、继续、重做、修订和旧路径；恢复运行不再
悄悄回到另一种权限。五底座新建会话中，Claude Code、Codex 与 OpenCode 通过各自原生权限/
沙箱协议接收所选档位；Grok Build 与 Kimi Code 通过官方 ACP v1 协商能力，但使用彼此隔离的
厂商 profile。Claude Code/Codex 成功跨进程恢复；OpenCode 没有跨进程 resume，continue 会按
同一档位降级为新会话；Grok Build 在未证明恢复后的 sandbox 与启动前校验前使用新会话交接；
Kimi Code 只在实时声明相应能力后使用 `session/resume` 或 `session/load`，并重新应用模式与
模型。UmaDev 不应通过字符串假装已经拥有能力。

这里仍有一个必须明示的运行时边界：`BaseSession` 当前没有在原会话内重协商启动权限的协议，
常驻底座会话会保留启动时的沙箱和工具许可。运行中通过 Shift+Tab 或 `/mode` 切换会立即更新
UmaDev 的实时审批决策，并使旧权限快照的常驻/Director 会话失配；下一次使用按新档位重建。
自动化已覆盖五底座 × 三档的新开语义、可证明的恢复语义，以及底座/规范工作区/权限档变化时
拒绝复用旧会话。Grok Build 因 ACP 尚未证明恢复后的 sandbox 生效而对三档持久恢复都 fail-closed；
Kimi Code 恢复后重新应用档位，Auto 保持 default 并由 UmaDev 本地策略中介普通审批。

成熟产品必须把“环境能力”和“审批自动化”拆成两个轴。Guarded 已允许启动本地端口、联网安装
和正常开发操作，但在敏感动作上保留确认；Auto 不再扩大环境能力，只减少普通底座审批等待。
Plan 才是明确的只读档。驱动必须把这三种语义真实传给底座，不能让界面档位与实际沙箱或审批
策略分裂。

## 5. 终端与交互成熟度

已经建立的关键不变量包括：单次进入/幂等退出备用屏、唯一 stdin 读取者、异步 OSC 回复、
Windows CJK/IME 保护、宽字符测量、有限重绘和可手动重绘。这些方向正确。

但是 GitHub Actions 上的 Linux/macOS/Windows 测试不能证明所有终端组合都没有渲染问题。
发布前仍需执行 [`TERMINAL_COMPATIBILITY.md`](TERMINAL_COMPATIBILITY.md) 中的人工矩阵，
至少覆盖 Windows Terminal/PowerShell、ConPTY、VS Code 终端、WezTerm、iTerm2、Terminal.app、
常见 Linux 终端、tmux、SSH、深浅主题、窗口缩放、中英文输入法和复制粘贴。没有这份带版本、
终端、OS 和截图/日志的签署记录，就不能承诺“各种终端零问题”。

本轮补齐或修正的用户可见问题还包括：

- 普通自然语言输入现在由所选底座模型在 fresh read-only child 中先做类型化语义判断，并且可
  双向覆盖关键词先验；Chat/Explain 在只读执行面回答，模型选择写入必须带精确合法的
  `authorization: "mutating"`，否则 fail-closed；确定性兜底不继承非法字段，只能识别当前用户
  文本中无歧义的窄范围请求。Plan 模式无法被模型越权放大。所有 Build 和
  Standard/Deep Debug 才进入 Director；旧计划、TODO、规范及项目文档不能替当前请求授权，
  模型不可用时的确定性兜底不能独自启动 Director、团队或完整构建 QC；
- Plan 下的 `/run`、`/goal` 和执行型恢复在 run lock、隔离分支、治理/工作流写入与底座会话
  之前结算为类型化 `Planned` 非执行结果，不能显示 `Done`；普通对话仍可做只读研究与规划；
- QuickEdit/Fast Debug 只做定向修改，并且只有在最后一次代码写入后观察到成功的定向验证才可
  完成；严格拒绝包装在 echo、shell 组合、重定向、fix/write/help/watch 模式、可写脚本名或成功
  验证后任意 shell 写入中的伪验证。工具与终态结果按 FIFO 对齐，Codex `outputDelta` 只显示进度、
  不结算验证，Windows 可执行后缀归一化。未验证写入以 `Failed` 收口。发生写入不等于进入
  Director，也不产生 full completion、完整构建卡或 Director 会话交接；
- 自然语言被提升为 Director 后，UI 会立即进入运行期输入协议：当前任务调整进入 steer，问题
  和未来任务按 FIFO 延后重新走模型路由；gate 上的问题使用独立只读查询立即回答，不推进或
  修订 gate。`GateOpened` 在 writer session 结束前只暂存，避免审批/恢复与尾部事件竞态；
  `取消/cancel` 会停止当前 run、清除底座原生 resume/session hand-back 并写入对话控制边界，
  但保留此前排队的 FIFO 输入。2 秒排空只是 UI 预算，未真实退出的任务仍保持写者所有权、
  丢弃迟到事件并阻止后续 FIFO 启动；部署任务 drop 时清理进程组。ClarifyGate 不再把问题或
  “稍后/later”任务写入澄清答案；压缩失败也携带会话代际，旧失败不能裁剪 `/clear` 或恢复后的
  新对话、也不能污染新熔断器；
- Director 的源码硬门、Blocked、Active/Pending/未完成计划、dirty final QC，以及预算耗尽后的
  残留都以带有界阻塞证据的 `Failed` 终态收口，不再被后续完成回执反写成 `Done`；只有机械
  干净的终结计划才可完成并把精确的底座原生 session id 交给常驻聊天；gate 暂停使用独立
  `Paused` 终态并显示 gate，不再打印“构建完成”；
- 复制成功提示只进入状态区并在约 2.5 秒后消失，不再占聊天记录；
- Grep/Glob 只在底座输出首个非空摘要行符合有界的显式计数语法时显示匹配数；仓库正文中的
  裸数字（包括曾被误报为 `3000000000000000` 个匹配的内容）不再被重新标注为统计值；
- `umadev doctor` 的成功诊断同时给出 TUI 入口和可直接执行的完整一次性命令，不再把脱离
  `umadev run` 的裸 `--backend` 参数展示成用户操作；当前架构文档统一表述为五底座；
- 顶层无参数启动只有在 stdin 与 stdout 同时为终端时才进入 TUI；stdout 重定向或任一输入/输出流
  被管道接走时只输出普通帮助，不启用 raw/备用屏，也不把终端控制帧写入文件。stderr 不承载渲染帧，
  因而不作为启动条件；
- `/init` 区分空项目和已有项目，探测技术栈/命令并以受控块更新指令文件；
- Claude Code 已在子 Agent 结算前门控主输出；OpenCode 已按 session 图和 SSE/状态复核等待
  子会话 settle；Codex 已按 `threadId` 隔离子线程原始事件，并把主线程
  `collabAgentToolCall` / `agentsStates` 转成权威 live-set 接入同一输出门控；
- 可恢复的 429/502/网络类底座失败使用有界、可见倒计时重试；若用户选择手动重发，失败结算
  会先清除旧去重键，因此第一次相同输入会真正发送，不再被误判为重复消息；
- `/pitfalls` 与 `/lessons` 分离，并显示时间、状态和无结果原因；
- `/lessions` 会指向正确的 `/lessons`；学习进度通知跟随三语界面，不再固定输出简体中文；
- 五底座新建会话的权限档与实际启动参数或协商能力一致；Claude Code/Codex 成功恢复，OpenCode
  continue 按原档位降级为新会话，Grok Build 使用安全的新会话交接，Kimi Code 只在声明后
  使用 `session/resume` 或 `session/load` 并重新应用模式/模型；常驻会话用权限
  快照和单调代际隔离旧任务，`/clear` 与同底座恢复会清除 steer/route/FIFO 瞬态状态；运行中
  切换底座启动级权限仍需重建会话；
- OpenCode 的探测、单发执行和持续会话启动共享 fail-closed 最低版本门。低于 `1.14.31`，或
  `--version` 无法按精确 semver 解析时，均明确拒绝并给出升级命令，避免旧版 `Task` 子代理绕过
  Plan 只读权限（上游 [issue #20549](https://github.com/anomalyco/opencode/issues/20549) / 修复
  [PR #23290](https://github.com/anomalyco/opencode/pull/23290)）；
- Claude Code 的只读硬边界是 `--permission-mode plan`；`--allowedTools` 按官方语义只是免审批
  清单，不是第二个沙箱，源码注释和产品说明已按此纠正；
- npm 升级只有在主包、平台包和真实二进制版本一致后才报成功；独立二进制升级则在官方同源
  URL、SHA-256、文件格式和原子替换通过后报成功，目前仍提示用户运行 `--version`，未在进程内
  复验新二进制的语义版本。

## 6. 代码质量、供应链与发布

已具备或在本地工作树补齐的企业级门禁：

- `cargo fmt --all -- --check`；
- workspace 全目标 `clippy -D warnings`；
- workspace 全目标测试和 doctest；
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps`，全 workspace 严格
  rustdoc 已清零并进入普通 CI 与 release 质量门；
- 声明 MSRV 的锁文件检查；
- 注释治理 `UD-CODE-006d` 只提示新增/恶化的长说明块和“注释远多于代码”，不使用粗暴注释
  配额；
- release tag 硬依赖同一提交的 Linux 全质量门和 macOS/Windows 测试，并配置五目标构建矩阵与
  npm 分发 smoke 门；
- release quality 在任何资产或 npm 包发布前，按精确提交重新检出并验证 Grok Build 与 Kimi
  Code 两份官方源码契约；普通 CI 与 tag release 还会直接下载精确的 Grok `0.2.101`
  Linux/macOS/Windows 官方二进制，校验逐平台 SHA-256 与版本后，在隔离 HOME 中执行真实 ACP
  认证边界握手；同时执行官网 lockfile 安装、生产依赖审计、lint 与 Pages 构建；
- RustSec 定期审计、Dependabot 和安全披露流程；
- GitHub Actions 完整提交 SHA 固定、最小权限；
- 发布前锁定 Cargo、tag、官网更新历史、npm 主包/平台包/知识包 manifest 及精确依赖版本；
- 五个 GitHub 二进制与五个 npm 平台包逐字节 SHA-256 对照；同 runner 架构的 JS 启动器执行
  `--version`，交叉架构构件仍明确等待实机运行；
- GitHub Release 先创建草稿，精确 18 个资产下载回验字节一致后才公开；公开版本重复运行只在
  内容完全一致时 no-op，不覆盖用户可能已下载的字节；
- npm 在首次发布前冻结七个 tarball，以 registry SHA-512 integrity 验明已有/新发精确版本，
  缺失版本先进入 `staging`，全部就绪后按依赖顺序提升 `latest` 且主包最后；官网部署硬依赖 npm；
- 上游模型 revision 与三个输入 SHA-256 同时固定；二进制、模型和 SPDX JSON SBOM 均生成
  SHA-256 sidecar 与 GitHub artifact attestation，npm 发布启用 provenance；GitHub Release 的
  精确资产清单因此为 18 项；tag 发布还会在生成 sidecar 和 npm 包前强制执行 macOS Developer
  ID/notarization 与 Windows Authenticode，任一签名凭据缺失都会在构建前失败；
- 知识库构建时只允许文本扩展名，排除隐藏缓存、符号链接和本地向量索引。
- 五底座支持面只由 `BACKEND_IDS` 定义；规范与 `RuntimeKind` 注释现已明确后者只是向后兼容的
  粗粒度 wire tag，不是主机枚举、provider 选择器或 Agent SDK 使用证明。OpenCode 保留
  `RuntimeKind::Openai` 仅为兼容债务，主机判断必须使用 base id 和 capability；
- 规范 clause 与普通内容 lint 使用不同命名空间：正式 `UD-CODE-*` 不再被普通检查复用，
  15 条 craft/lint 迁到 `UG-LINT-001..015`，旧策略配置仅作为兼容别名读取；
- `UD-CODE-003` 的 API 对齐 JSON 向量已经接入 runner；全仓扫描覆盖 YAML、Shell、CSS、
  MJS/HTML 等实际文件类型，并排除生成目录 `out/`；
- 官网当前事实统一为 34 条 clause、113 条治理规则和“400+”知识文件，不再用易漂移的精确语料
  数量；移除了未被渲染却伪报版本、模块数和 94% 通过率的诊断 HUD，增加局部与全局错误恢复页，
  中英文切换同步更新文档语言，selection token 与装饰 glyph 也回归设计 token/CSS 图形；Next
  16.2.9 的传递 PostCSS 已覆盖到修复版 8.5.19，干净 `npm ci` 与官方 registry 生产依赖审计为
  0 漏洞。

仍未完全收口：

1. 仓库没有 `CODEOWNERS`，而真实负责人不能由工具臆造；
2. npm trusted publisher、GitHub Environment 保护和官网/CDN 权限属于外部配置，代码无法证明
   已正确启用；
3. 当前完整自扫描已经收敛到 247/247 文件、0 条候选命中；此前 29 条信号中的真实问题已修复，
   测试向量、扫描器示例、配置与高熵字面量误报已按局部证据消除。零命中只证明当前仓库没有
   触发已启用规则，不能被包装成“代码没有缺陷”；fixture 策略和人工维护性审阅仍需持续；
4. 向量依赖链仍带 `paste` unmaintained 警告，当前无可直接升级的修复版，应持续跟踪而非
   把它描述成已解决漏洞；
5. Codex 子 Agent 归因已按当前 app-server schema 覆盖 `threadId`、`collabAgentToolCall` 和
   `agentsStates`；旧版本缺少归属字段时会 fail-open。由于协议随本机 Codex 版本演进，发布矩阵仍需
   覆盖声明支持的当前/上一版本，避免字段漂移重新造成内容穿插或门控不释放；
6. GitHub 草稿、npm `staging`/`latest` 与官网依赖顺序已经消除“半个主包被 latest 看见”的窗口，
   且失败后可按 digest 幂等续跑；但 GitHub、npm、Pages 是三个外部系统，不存在分布式事务。
   GitHub 已公开而 npm 尚未提升、或 npm 完成而官网部署失败时仍依靠重跑收敛，不能承诺自动回滚；
7. 模型下载已同时固定上游 revision 与 config/tokenizer/原始 safetensors 的仓库内 SHA-256；
    量化后的 sidecar 和 attestation 仍是本次构建输出记录。量化环境的 pip 依赖固定了版本，但
    尚未锁定 wheel hashes，构建工具链还不是完全可复现环境；
8. 发布矩阵已对二进制与 npm 包做 SHA-256 同一性校验，并在 runner 与目标架构一致时经 JS
    启动器执行 `--version`；当前源码另已在本机原生 Linux/arm64 Docker 内构建并执行，但最终
    tag 的 Linux arm64 交叉构件及与 macOS runner 不同架构的其它构件仍只做哈希验证，不能
    表述为“五个平台最终发布资产都已执行验明”；
9. 独立更新器对同源 `.sha256` 的核对可以发现传输损坏或单个对象被替换，但不能抵御发布权限
    被接管后 binary 与 sidecar 同时替换。客户端目前不验证 GitHub attestation 或独立签名；更强的
    发布真实性需要客户端验证 Sigstore/TUF，或至少验证离线密钥签署的 release manifest；
10. Developer ID/notarization 与 Authenticode 已进入 tag 发布的 fail-closed 工作流；当前仓库仍未
    配置所需 Apple/Windows 证书 secrets，因此只有真实 tag run 通过后才能证明发布者身份链完成；
11. npm 更新仍缺 Windows IDE、杀毒软件和多个真实终端共同持有 `umadev.exe` 的人工签署。
    Windows CI 现在会启动一个真实复制的 `.exe` 形成 OS 文件占用，让模拟包管理器返回 0，
    并要求更新器识别版本分裂、失败且输出精确修复；这证明运行映像占用边界，但不能代表每种
    IDE/杀毒软件注入方式；
12. 四个历史热点已完成首轮真实拆分并收紧 LOC 门；`App` 的超宽状态、TUI 事件循环、UI 渲染和
    lessons 存储仍有较大变更半径，后续应继续沿既有边界下降而不能重新聚合。

## 7. 发布前必须满足的验收条目

- [x] 当前合并工作树的精确 all-features 全目标回归为 5,153 passed、0 failed、7 ignored（总计
      5,160 项；6 项外部 live-base 条件测试与 1 项手工性能基准按设计忽略）；底座超时回归已从忽略慢测
      升为普通 E2E，验证重试有界、不会假报完成，并会原子覆盖写出明确标注“非底座生成”的澄清文件；测试
      HOME/XDG 与本地向量模型目录已经密封，普通回归不会再继承用户机器上的真实 embedding 模型。
      workspace doctest、严格 rustdoc、`cargo fmt --all -- --check`、全特性 clippy（`-D warnings`）和
      Rust 1.88 locked check 均通过；
- [x] macOS arm64 本地执行 `cargo build --release --locked --bin umadev --features vector-local` 通过，
      `target/release/umadev --version` 返回 `umadev 1.0.56`；这只验证当前平台产物；
- [ ] `1.0.56` 的 Linux/arm64 与 Linux/x64 最终资产仍需 release CI 构建并实际验明；同一功能
      工作树在版本提升前已于 Debian bookworm / Rust 1.88 原生 ARM Linux 构建并运行为
      `umadev 1.0.55`，但旧版本证据不能冒充新 tag 资产、glibc 2.31 基线或 Linux x64 实机；
- [x] RustSec 扫描 490 个依赖，0 个已知漏洞；仍保留 1 个获准的维护性警告：间接依赖
      `paste 1.0.15` 已停止维护，应继续跟踪上游替换；
- [x] 官网 `npm run lint` 与 `npm run build` 通过；这验证当前 macOS 工作树的静态构建，
      干净 `npm ci` 与 npm 官方 registry 的生产依赖审计也通过且为 0 漏洞；这不等价于跨浏览器、
      跨终端和三操作系统实机签署；
- [x] 自进化状态机由独立测试覆盖：独立去重、两次复发、精确成功、同错失败、异错 unknown、
      legacy 隔离、时间序列化、`/pitfalls`/`/lessons` 不重叠；
- [x] 无参数 TUI 的纯判定覆盖 stdin/stdout TTY 组合；stdout 重定向不进入 raw/备用屏；
- [x] OpenCode 最低安全版 `1.14.31` 在探测、单发与持续会话入口统一执行，定向回归覆盖低于、
      等于、高于、标签/前后缀、精确版本 prerelease 和不可解析输出；
- [x] 权限自动化矩阵覆盖五底座 Plan/Guarded/Auto：Claude、Codex、OpenCode 的新开/恢复
      共用并核对同一权限档，Grok Build 三档新开参数逐项锁定且未获生效 sandbox 证明时拒绝
      持久恢复，Kimi Code 锁定 plan/default 与本地中介 Auto 并验证恢复重放；底座、规范工作区
      或权限档任一变化都会使常驻/Director 会话失配并重建；
- [x] macOS arm64 上用 release 版 `umadev 1.0.56` 完整复跑 npm smoke 通过；脚本覆盖
      npm/pnpm/bun 更新执行、yarn 布局的所有权识别、主包/平台包/二进制分裂与假成功拒绝，
      但不替代各平台真实全局安装和 Windows 文件占用验证；
- [x] 官方 Grok Build `0.2.101` 在 macOS arm64 的真实验收通过：Plan 父会话与只读 fork
      精确返回关联 token 且工作区零改动；Auto 实际写文件、运行 Rust 并完成 `127.0.0.1`
      临时端口监听/回连；原生 prompt queue 排空两轮；服务器权威后台进程可列出且只能按归属
      停止；Folder Trust 拒绝保持项目配置 gated。普通 CI 与 tag release 另已定义哈希固定的
      Linux/macOS/Windows 精确发布二进制无登录 ACP 门，但仍需第一次远程三平台运行证明这些行；
- [x] 官方 npm `@moonshot-ai/kimi-code@0.26.0` 已安装到隔离目录并在 macOS arm64 运行真实 ACP
      无登录契约：精确版本/身份门通过，隔离 HOME 中在 `session/new` 前返回只含终端登录指引的
      失败，未打开浏览器、未改写项目。机器 PATH 中同名的旧 Python `kimi-cli 0.53` 也被真实探测
      为冲突并拒绝误启动；普通 CI 与 tag release 会在 Linux/macOS/Windows 重新安装同一精确包
      执行该契约，仍需第一次远程三平台运行证明这些行；
- [x] 当前工作树用刚编译出的 `umadev ci --report-only --project-root .` 自扫描 247/247 个文件、
      0 个不可读、0 个命中文件、0 条治理命中；collect-all 与实时门共用 Policy、上下文、别名和
      fail-open 语义。`report-only` 的退出码仍只表示扫描完成，不应被误解为独立的合规证明；
- [ ] Windows 实机验证 IDE/杀毒软件/多终端占用、PATH 多副本、PowerShell 与独立二进制回滚提示；
      Windows CI 已增加真实运行中 EXE 的锁定回归，但尚不能替代这项人工签署；
- [ ] 按终端人工矩阵签署，不以单个 CI 绿灯替代实机兼容性；
- [x] 全 workspace/all-features strict-rustdoc 清零，并加入普通 CI 与 release 质量门；
- [x] all-features runtime 回归使用隔离 HOME/XDG、显式空模型目录和受控测试模型，不再意外拉起
      多个真实 embedding 进程；真实模型兼容性继续作为显式集成/发布抽检，而不是普通单元测试副作用；
- [x] GitHub Release、npm、官网和更新历史已有显式依赖、digest 核对与可恢复顺序：GitHub 草稿
      验明后公开，npm 七包 staging 后主包最后提升 latest，官网最后部署；外部系统间仍无分布式事务；
- [x] release workflow 硬依赖同一提交的 Linux/macOS/Windows 质量门；
- [ ] macOS Developer ID/notarization 与 Windows Authenticode 的强制流水线已经实现；仍需配置
      证书 secrets，并以真实 tag run 验明签名、公证、时间戳和最终下载制品。本机 arm64 release
      已用有效 Developer ID Application 身份完成 Hardened Runtime + Apple 时间戳签名且
      `codesign --strict` 通过，但 Gatekeeper 仍报告 `Unnotarized Developer ID`，不能替代公证；
- [x] crate 依赖方向、生产图无环、foundation 反向依赖与四热点精确 LOC ratchet 已进入测试；
- [x] 四个历史热点完成首轮真实拆分：App/规则测试与完整扫描迁出，Director 恢复/契约测试和 TUI
      组合根已分层，四个主文件均显著下降且 ratchet 同步收紧；
- [ ] 发布后从干净环境抽检每个平台的安装、`--version`、启动、`update` 与回滚提示。

## 8. 最终判断

UmaDev 已经实现了“模拟真实开发团队的 Agent”最关键的系统骨架，而且单写者执行、只读并行
评审、类型化工件和机械地板这组选择是正确的。用户现在遇到的大量问题主要不是产品理念错，
而是进入成熟期后暴露出的边界闭环：权限是否真传递、协议是否真 settle、升级是否真一致、
记忆是否真验证、终端状态是否真恢复、文档是否真反映代码。

本次修复是在收这些边界，而不是再堆功能。等第 7 节全部打勾、治理信号完成局部性收敛、
跨终端人工矩阵有签署记录、发布身份链补齐后，
才适合把产品称为“企业级成熟”；在那之前，最准确的对外表述是：
**核心能力完整，架构方向正确，正在完成跨平台与运维级硬化。**
