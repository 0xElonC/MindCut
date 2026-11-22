# 创作页(AI 短剧)开发文档

状态:剧本、分镜两个阶段已实现;角色与场景、生成、预览与交付三个阶段**尚未开发**。本文档把设计和开发思路留档,E-Flow 里的原计划已取消,之后按本文重新立项。

- 设计稿(6 个画板:入口、剧本、角色与场景、分镜、生成队列、预览与交付):https://claude.ai/artifact/EyJrf3aZEUmNauw6giGygZ
- 已提交:`614b6166` concat-drama crate;`850b99da` 数据层、队列与分镜阶段;`c708fcce` 剧本阶段(未推送)。
- 剧本阶段只通过了 clippy 和测试,**没有在窗口里运行过**。

## 1. 要做什么

在 MindCut 里做 AI 短剧创作:剧本 → 角色与场景 → 分镜 → 生成(文生图、图生视频、配音)→ 预览与交付,满意后"去剪辑"把镜头铺到时间线。

已定的决定(用户,2026-11-18/19):

- 只用云端 API;竖屏 9:16;配音必须有(云端 TTS,不用本地 chatterbox)。
- 创作页是独立的一整屏,和启动页、剪辑页并列,标题栏用"创作/剪辑"开关切换。
- 五个阶段:剧本、角色与场景、分镜、生成、预览与交付。**分镜是核心页**,镜头卡是所有生成的单位。
- 服务商**还没选**,设计稿里写的是"服务商 A/B/C"。

## 2. 现状(已有代码)

| 位置 | 内容 |
|---|---|
| `src/crates/concat-drama/src/model.rs` | Story / Scene / Shot / Character / Slot / Take / Voice / Phase |
| `src/crates/concat-drama/src/store.rs` | story.json 存在项目文件夹里,原子写入,加载时把陈旧的"进行中"改成失败 |
| `src/crates/concat-drama/src/prompt.rs` | 由故事生成请求,锁定角色的定妆图作为参考图 |
| `src/crates/concat-drama/src/jobs.rs` | Queue + Backend trait + Rehearsal 假后端(写 SVG 和占位文件) |
| `src/crates/concat-drama/src/script.rs` | 剧本拆分:"场景:/Scene:/#"开新场景,"名字:台词"是说话,其他行是一个镜头;时长按每秒约 4 字估算,有画面最少 5 秒最多 15 秒;改剧本再拆时文字没变的镜头原样保留 |
| `src/crates/concat/src/drama.rs` | 创作页控制器,运行在事件循环线程 |
| `src/crates/concat/ui/story/story.slint` | 创作页界面(793 行,全部阶段在一个文件) |
| `src/crates/concat/src/agent.rs`、`src/crates/concat-agent` | 现有的 Agent 面板,复用本机 Codex 的 app-server,通过 concat-mcp 的工具改时间线 |

必须保持的原则:

- 生成物都是 **Slot + take**,重新生成不覆盖旧的,可切换。
- 不要把空故事写到加载失败的 story.json 上。
- 密钥只存本机,不进 story.json 和项目文件夹。

## 3. 剩余阶段的设计

### 3.1 角色与场景(设计稿 Cast 画板)

三栏:左资产库(角色/场景/道具,带状态点);中详情(定妆图、三视图、外貌描述、参考图、声线、锁定/解除锁定、重新生成、换成我自己的图);右导演助手。

流程:生成 3 张候选定妆图 → 选一张 → 生成正面/侧面/背面三视图 → 锁定。锁定的角色之后每个镜头都带它的参考图。顶部提示"某某还没有定妆图",按钮"下一步:分镜"。声线克隆本期不做。

### 3.2 生成(Generate 画板)

顶部统计:镜头数、完成、进行中、失败、已用/预计费用(按服务商报价估算)。筛选:全部/进行中/失败/待生成。逐镜头一行,三格状态:首帧图、视频、配音;失败行显示原因和重试;重试失败项、暂停队列。

右栏"服务商与密钥":三类服务商各选一个、并发数(1/3/5)、失败自动重试(关/最多 2 次),并说明密钥只存本机、请求从本机直发服务商。

### 3.3 预览与交付(Review 画板)

左侧 9:16 预览(播放、进度、镜头徽标、字幕);右侧"交付前检查"(失败镜头、配音比画面长并给差值、已就绪数)和"去剪辑时"三个选项(自动铺到时间线、配音独立音轨、台词生成字幕);底部镜头条可拖动排序、选中重生成。按钮:导出预览 MP4、去剪辑。

镜头带"来自创作页"标记,剪辑页里可回创作页重生成,新版本替换到原位置。

### 3.4 导演助手

Cast 与分镜画板右侧 340px 的助手栏。**复用现有 Codex Agent**,不另起一套:仿照编辑器工具(concat-mcp → concat-api → 窗口)新增 `story_*` 工具(读故事、改剧本与分镜、改角色、请求生成、读队列状态)。

要点:Agent 与界面同时编辑时两边都不丢;生成走已有队列,不绕过暂停与并发设置。

## 4. 开发思路与顺序

先**地基串行**,再**三条线路并行**,经**关卡**检查后做**集成**,最后**窗口走查**。E-Flow 的"多角色并行"规则:任务写 `dependsOn` 和 `scope`,各线路写入范围互不重叠,共享文件只放地基或集成。

1. 地基:把 `story.slint`、`drama.rs` 按阶段拆成独立文件,每个阶段有自己的 State global;资产模型;服务商与密钥设置;队列控制。
2. 并行三线:角色与场景;导演助手;生成与交付(交付前检查逻辑、生成界面、预览与交付界面)。
3. 关卡:检查三线衔接。
4. 集成:去剪辑、回创作页重生成、文案并入并翻译。
5. 窗口走查(用 Rehearsal 假后端从剧本走到"去剪辑")。

文案冲突的处理:26 个 locale 是共享文件,并行期间各线路把新文案写到 `src/crates/concat/locales/pending/<线路>.json`,集成阶段统一并入并翻译其余 24 种语言。

## 5. 服务商选择(未定)

价格来自第三方汇总博客,来源互相矛盾,**以官方价格页为准**。

- 文生图(要角色一致):Nano Banana(Gemini 系,多参考图)、Seedream、Flux 2 Pro、GPT Image 2。建议优先 Nano Banana。
- 图生视频:Kling、Seedance(有更便宜的 Fast 档)、Hailuo、Vidu、Veo。建议竖屏短剧优先 Kling 或 Seedance。
- 配音:MiniMax(中文好)、Azure(有免费额度)、ElevenLabs。建议先 MiniMax。
- 也可以接聚合网关(统一的异步图片/视频接口),一家覆盖多个模型,省适配器;需自行评估价格和可靠性。
- 应用里已有 HTTP 客户端 `ureq`(`concat-host`、`concat-speech`),适配器可直接用。

## 6. 参考项目:Toonflow

https://github.com/HBAI-Ltd/Toonflow-app (MIT,TypeScript/Bun/Electrobun,服务端加 Web 前端)。**不能直接整合**(技术栈不同),只借设计思路,自行用 Rust 实现,并在代码注释里注明出处。它的旧版本曾用 AGPL-3.0 / Apache-2.0,若要参考某个旧版本的代码,先核对该版本的许可证。

可借鉴:

- **服务商能力声明**(`packages/providers/types.d.ts`):每个模型声明图片模式(文生图/单图/多参考)、视频模式(单图/首尾帧/多参考)、时长与分辨率允许的组合、声线列表;界面和请求按能力声明构造,不支持的组合提交前就拒绝。
- **异步任务可恢复**:任务被服务商接单后立即保存恢复数据(任务 id 等,不含密钥),重启后继续取结果。我们的 store 现在会把陈旧的"进行中"改成失败,要区分有无恢复数据。
- **工作流提示词**(`packages/skills/workflow`):剧本到分镜的做法,可用于 Agent 说明文字。
- 它仓库里没有具体服务商(Seedance 等)的适配代码,只有接口定义。

## 7. 以后再做(不在第一版)

用户也想要 Toonflow 的:**无限画布、3D 预演、插件市场**。三者都是独立大项目:画布交互会改变"五个阶段"的结构;3D 预演主要是素材资源工作量;插件市场要先设计扩展接口、安全与分发。

建议先完成主线,再按"插件扩展接口 → 无限画布 → 3D 预演"逐个立项,立项前先读实现并写范围与风险说明。

## 8. 原计划的任务清单(已取消,可直接复用)

以下是计划被取消前的任务,顺序即执行顺序。写入范围是并行放行用的读写声明。

### 1. 把创作页界面和控制器按阶段拆成独立文件(`split-story-files`,角色 build)

- 依赖:无
- 目标:这是并行前的地基。目前 ui/story/story.slint(793 行)和 src/drama.rs(518 行)包住了所有阶段,后面三条线路同时改会互相冲突,所以先拆开,行为保持不变。要做:1) story.slint 拆成 storyboard.slint、script.slint(把现有的分镜与剧本阶段原样移过去)和新的空壳 cast.slint、generate.slint、review.slint、assistant.slint;每个阶段有自己的 global(CastState、GenerateState、ReviewState、AssistantState)放在各自文件里,在 app.slint 同 StoryState 一样导出,StoryScreen 只负责按阶段路由、阶段条和阶段间跳转。2) drama.rs 改成 drama/mod.rs,另建 cast.rs、generate.rs、review.rs、assistant.rs,每个只有 `pub(super) fn install(app)` 空壳并由 mod.rs 的 install 调用;Controller 与 with() 改为 pub(super),让各阶段模块能读写故事状态。3) concat-drama 新建空模块 cast.rs、voice.rs、checks.rs 并在 lib.rs 注册。4) 若 concat-api、concat-mcp 要依赖 concat-drama(导演助手工具要用),现在就把依赖加上,让 Cargo.lock 只在这里变。5) 角色与场景、分镜两个阶段的版面右侧各留一个 340px 的助手栏插槽(默认隐藏,由 AssistantState 控制显示)。为什么:并行任务的写入范围不能重叠,共享文件只能放地基里。
- 验收:
  - cargo clippy 与 cargo test(concat、concat-drama)通过,原有 7 个剧本拆分测试仍通过
  - story.slint 与 drama.rs 不再包含阶段具体实现,新增文件列表写进 done 摘要
  - 剧本、分镜阶段行为与拆分前一致(不要求启动窗口,代码核对即可)
  - 四个 State global 都已在 app.slint 导出

### 2. 扩展场景与道具资产模型(`asset-model`,角色 build)

- 依赖:split-story-files
- 目标:设计稿资产库有角色、场景、道具三类。给 concat-drama 的 Scene 加场景图 Slot,新增 Prop(道具,含名称、描述、图 Slot),prompt 生成时把场景图和镜头里出现的道具图作为参考图带上;旧 story.json 仍能读。
- 验收:
  - 旧格式 story.json 能原样加载(有测试)
  - 场景图、道具图的生成请求带上描述和参考图(有测试)
  - cargo clippy -p concat-drama 和 cargo test -p concat-drama 通过

### 3. 建立服务商与密钥设置(`provider-settings`,角色 build)

- 依赖:split-story-files
- 目标:新增服务商配置:文生图、图生视频、配音三类各选一个服务商,填密钥,并有并发数(1/3/5)和失败自动重试(关/最多2次)。密钥只存本机(沿用项目里已有的本机密钥或偏好存放方式,先查再做),绝不写进 story.json 或项目文件夹。Backend 按 Kind 分发到对应服务商;没配密钥时回退 Rehearsal 并明确标成演示。服务商清单做成可扩展的注册表,本任务只放 Rehearsal 一项,不接真实服务商。 另外给 Backend trait 加列声线与试听两个方法(带默认实现,Rehearsal 返回假声线与一段静音),让后面的声线任务只用不改 trait。 服务商注册表里每个模型要声明能力:类型(图/视频/配音)、图片支持的模式(文生图/单图参考/多图参考及参考图上限)、视频支持的模式(单图/首尾帧/多参考)、允许的时长与分辨率组合、配音的声线列表。界面和请求构造都按能力声明来,不支持的组合在提交前就拒绝并给出原因。异步任务(视频等)被服务商接单后,立即把恢复数据(任务 id 等,绝不含密钥)保存下来。 参考设计来自开源项目 Toonflow(MIT 许可证,https://github.com/HBAI-Ltd/Toonflow-app 的 packages/providers/types.d.ts):只借设计思路,自己用 Rust 实现,不复制代码;在代码注释里注明出处。
- 验收:
  - 密钥不出现在 story.json、日志和项目文件夹里(有测试或代码核对)
  - 按 Kind 分发到不同 Backend 有测试
  - 并发数、自动重试设置重启后保留
  - 模型能力声明有测试:不支持的时长/分辨率/参考图数量在提交前被拒绝并带原因
  - 异步任务接单后的恢复数据被保存,且其中不含密钥(有测试)

### 4. 补全生成队列控制(`queue-controls`,角色 build)

- 依赖:split-story-files, asset-model, provider-settings
- 目标:队列支持暂停与继续、重试失败项、失败自动重试(次数来自设置)、视频必须等首帧完成后才排队、配音与画面互相独立;并按服务商报价估算整体费用与已用费用(报价是可配置的估算值)。 重启应用时,已被服务商接单、状态为进行中的任务要用保存的恢复数据继续取结果,而不是像现在这样一律改成失败(现有 store 加载时会把陈旧的“进行中”改成失败,要区分有恢复数据和没有的)。
- 验收:
  - 暂停后不再取新任务,继续后恢复(有测试)
  - 自动重试到次数上限后停在失败状态并带原因(有测试)
  - 视频任务在首帧未完成时不会开始(有测试)
  - 费用估算函数有测试
  - 有恢复数据的进行中任务在重新加载后继续取结果并完成(用 Rehearsal 模拟,有测试)
  - 没有恢复数据的陈旧进行中任务仍改为失败并带原因(有测试)

### 5. 做角色与场景阶段界面(`cast-stage-ui`,角色 cast)

- 依赖:split-story-files, asset-model, provider-settings, queue-controls
- 目标:按设计稿 Cast 画板做三栏:左资产库(角色/场景/道具分组,带状态点),中详情(定妆图、外貌描述、锁定/解除锁定、重新生成定妆图),右暂不做导演助手只留位置。顶部显示“某某还没有定妆图”提示和“下一步:分镜”。新增、改名、删除角色/场景/道具,编辑自动保存。 新增的界面文案不要改 src/crates/concat/locales 下的任何 locale 文件(26 个文件是共享的,并行时会冲突);写进本线路自己的待合并文件 src/crates/concat/locales/pending/cast.json,格式 {"key":{"en":"...","zh-Hans":"..."}},key 用 story.<区域>.<名字>。最后由 story-locales 任务统一并入 locale 并翻译。合并前界面上会显示 key,这是预期的。
- 验收:
  - 阶段条第 2 步不再是占位,三类资产都能新增、编辑、删除
  - 锁定状态与定妆图缺失提示正确
  - cargo clippy 与 cargo test(concat、concat-drama)通过
  - 中文文案已写,其余语言暂用英文
- 写入范围:`src/crates/concat/ui/story/cast.slint`, `src/crates/concat/src/drama/cast.rs`, `src/crates/concat-drama/src/cast.rs`, `src/crates/concat-drama/src/voice.rs`, `src/crates/concat-drama/src/prompt.rs`, `src/crates/concat/locales/pending/cast.json`
- 限制:只改 scope.writes 里的文件;需要的共享改动(trait、lib.rs 注册)已在地基任务做好,缺了就 plan block 说明,不要自己改共享文件

### 6. 支持导入参考图与自己的定妆图(`cast-reference-images`,角色 cast)

- 依赖:cast-stage-ui
- 目标:角色可添加多张参考图;“换成我自己的图”可用自己的图片替代定妆图。导入的文件复制进项目 assets,story.json 只存相对路径;定妆图作为候选 take 保留,不覆盖已有的。 新增的界面文案不要改 src/crates/concat/locales 下的任何 locale 文件(26 个文件是共享的,并行时会冲突);写进本线路自己的待合并文件 src/crates/concat/locales/pending/cast.json,格式 {"key":{"en":"...","zh-Hans":"..."}},key 用 story.<区域>.<名字>。最后由 story-locales 任务统一并入 locale 并翻译。合并前界面上会显示 key,这是预期的。
- 验收:
  - 选图后文件进入项目 assets 且路径为相对路径(有测试)
  - 替换定妆图后旧的 take 仍在可切换
  - 参考图会随该角色的出图请求发出
- 写入范围:`src/crates/concat/ui/story/cast.slint`, `src/crates/concat/src/drama/cast.rs`, `src/crates/concat-drama/src/cast.rs`, `src/crates/concat-drama/src/voice.rs`, `src/crates/concat-drama/src/prompt.rs`, `src/crates/concat/locales/pending/cast.json`

### 7. 实现候选定妆图、三视图与锁定流程(`portrait-turnaround-lock`,角色 cast)

- 依赖:cast-stage-ui, cast-reference-images
- 目标:按设计稿:生成 3 张候选定妆图,选中一张后生成正面/侧面/背面三视图,然后锁定;锁定的角色之后每个镜头都带它的参考图。 新增的界面文案不要改 src/crates/concat/locales 下的任何 locale 文件(26 个文件是共享的,并行时会冲突);写进本线路自己的待合并文件 src/crates/concat/locales/pending/cast.json,格式 {"key":{"en":"...","zh-Hans":"..."}},key 用 story.<区域>.<名字>。最后由 story-locales 任务统一并入 locale 并翻译。合并前界面上会显示 key,这是预期的。
- 验收:
  - 候选 take 可选择,选中后才能请求三视图(有测试)
  - 三视图请求以选中的定妆图为参考(有测试)
  - 锁定的角色参考图出现在含该角色镜头的请求里(沿用已有测试并补覆盖)
- 写入范围:`src/crates/concat/ui/story/cast.slint`, `src/crates/concat/src/drama/cast.rs`, `src/crates/concat-drama/src/cast.rs`, `src/crates/concat-drama/src/voice.rs`, `src/crates/concat-drama/src/prompt.rs`, `src/crates/concat/locales/pending/cast.json`

### 8. 实现声线选择与试听(`voice-selection`,角色 cast)

- 依赖:cast-stage-ui, portrait-turnaround-lock
- 目标:Backend 增加列出声线和试听接口;角色详情里选声线(试听、语速、已选标记);镜头配音按台词说话人的声线生成,旁白用默认声线。声线克隆本期不做,界面不放入口。 新增的界面文案不要改 src/crates/concat/locales 下的任何 locale 文件(26 个文件是共享的,并行时会冲突);写进本线路自己的待合并文件 src/crates/concat/locales/pending/cast.json,格式 {"key":{"en":"...","zh-Hans":"..."}},key 用 story.<区域>.<名字>。最后由 story-locales 任务统一并入 locale 并翻译。合并前界面上会显示 key,这是预期的。
- 验收:
  - 列声线与试听走 Backend 接口,Rehearsal 有假数据(有测试)
  - 配音请求使用台词说话人的声线和语速(有测试)
  - 未选声线的角色在阶段提示里出现
- 写入范围:`src/crates/concat/ui/story/cast.slint`, `src/crates/concat/src/drama/cast.rs`, `src/crates/concat-drama/src/cast.rs`, `src/crates/concat-drama/src/voice.rs`, `src/crates/concat-drama/src/prompt.rs`, `src/crates/concat/locales/pending/cast.json`

### 9. 给 Agent 加创作页工具集(`story-agent-tools`,角色 assistant)

- 依赖:split-story-files, asset-model, provider-settings, queue-controls
- 目标:导演助手复用现有 Codex Agent(src/crates/concat-agent、src/crates/concat/src/agent.rs),不另起一套。先读懂编辑器���具(concat-mcp tools.rs、concat-api、concat-server)是怎么把 Agent 调用送到窗口的,按同样方式新增 story_* 工具:读故事(story_read)、改剧本与分镜(场景、镜头、台词、景别、运镜)、增删改角色与外貌描述、设定风格、请求生成某个角色或镜头的某一项并读队列状态。改动必须经窗口的故事状态落地,不能和界面自动保存互相覆盖;改动在界面上实时可见。Agent 的说明文字里写清创作页的工作方式(只改故事,不动编辑器时间线,除非用户在剪辑模式)。 写 Agent 说明文字(创作页工作方式、剧本如何拆分镜)时可参考 Toonflow(MIT)的 packages/skills/workflow 里的工作流文档,借思路自行改写,注明出处。
- 验收:
  - story_* 工具出现在 MCP 工具列表,每个有说明和参数 schema(有测试)
  - Agent 改一个镜头后界面立即显示,界面里同时在编辑别的镜头时两边的修改都不丢(有测试)
  - 没有打开故事的项目调用 story_* 返回清楚的错误,不会创建空故事覆盖已有的
  - 请求生成走已有队列,不绕过暂停和并发设置
  - cargo clippy 与 cargo test(concat-mcp、concat-api、concat-drama)通过
- 写入范围:`src/crates/concat-mcp/src/tools.rs`, `src/crates/concat-mcp/src/schema.rs`, `src/crates/concat-api/src/story.rs`, `src/crates/concat-api/src/lib.rs`, `src/crates/concat/src/agent.rs`, `src/crates/concat/src/drama/assistant.rs`, `src/crates/concat/ui/story/assistant.slint`, `src/crates/concat/locales/pending/assistant.json`
- 限制:只改 scope.writes 里的文件;concat-api、concat-mcp 对 concat-drama 的依赖已在地基任务加好

### 10. 做导演助手面板(`director-assistant-pane`,角色 assistant)

- 依赖:split-story-files, asset-model, provider-settings, queue-controls, story-agent-tools
- 目标:按设计稿 Cast 和分镜画板右侧的「导演助手」栏(340px):对话消息、工具调用状态(例如“生成陈默定妆图 · 3 张候选”带转圈)、输入框、快捷建议芯片、新对话按钮。在角色与场景、分镜两个阶段显示,复用现有 Agent 面板的会话、模型选择和状态页(未安装 Codex 等),使用 story_* 工具。对话归属当前创作项目。 新增的界面文案不要改 src/crates/concat/locales 下的任何 locale 文件(26 个文件是共享的,并行时会冲突);写进本线路自己的待合并文件 src/crates/concat/locales/pending/assistant.json,格式 {"key":{"en":"...","zh-Hans":"..."}},key 用 story.<区域>.<名字>。最后由 story-locales 任务统一并入 locale 并翻译。合并前界面上会显示 key,这是预期的。
- 验收:
  - 角色与场景、分镜阶段右侧出现助手栏,可发消息并看到流式回复和工具调用状态
  - 对助手说“把陈默改成二十出头、瘦、连帽衫”,角色外貌描述被更新并触发定妆图生成(Rehearsal 下)
  - 未安装或未登录 Codex 时显示现有的状态页提示,不报错退出
  - 中文文案已写,其余语言暂用英文
  - cargo clippy 与 cargo test(concat)通过
- 写入范围:`src/crates/concat-mcp/src/tools.rs`, `src/crates/concat-mcp/src/schema.rs`, `src/crates/concat-api/src/story.rs`, `src/crates/concat-api/src/lib.rs`, `src/crates/concat/src/agent.rs`, `src/crates/concat/src/drama/assistant.rs`, `src/crates/concat/ui/story/assistant.slint`, `src/crates/concat/locales/pending/assistant.json`
- 限制:只改 scope.writes 里的文件;concat-api、concat-mcp 对 concat-drama 的依赖已在地基任务加好

### 11. 实现交付前检查逻辑(`delivery-checks`,角色 delivery)

- 依赖:split-story-files, asset-model, provider-settings, queue-controls
- 目标:在 concat-drama 里写纯逻辑检查:失败镜头、缺首帧或缺视频、配音比画面长(给出各镜头的差值)、已就绪镜头数;并提供“自动延长画面到配音长度”的计算。
- 验收:
  - 每类问题都有测试,包含配音长于画面的差值
  - 已就绪数与问题清单互不重叠(有测试)
  - 不修改 story 数据,只返回检查结果
- 写入范围:`src/crates/concat-drama/src/checks.rs`, `src/crates/concat/ui/story/generate.slint`, `src/crates/concat/ui/story/review.slint`, `src/crates/concat/src/drama/generate.rs`, `src/crates/concat/src/drama/review.rs`, `src/crates/concat/locales/pending/delivery.json`
- 限制:只改 scope.writes 里的文件;共享文件缺什么改动就 plan block 说明

### 12. 做生成阶段界面(`generate-stage-ui`,角色 delivery)

- 依赖:split-story-files, asset-model, provider-settings, queue-controls, delivery-checks
- 目标:按设计稿 Generate 画板:顶部统计(镜头数、完成、进行中、失败、已用/预计费用),筛选(全部/进行中/失败/待生成),逐镜头一行显示首帧图、视频、配音各自的状态和进度,失败行显示原因和重试,另有重试失败项、暂停队列;右侧是服务商与密钥面板(三类服务商、并发数、自动重试、密钥只存本机的说明)。 新增的界面文案不要改 src/crates/concat/locales 下的任何 locale 文件(26 个文件是共享的,并行时会冲突);写进本线路自己的待合并文件 src/crates/concat/locales/pending/delivery.json,格式 {"key":{"en":"...","zh-Hans":"..."}},key 用 story.<区域>.<名字>。最后由 story-locales 任务统一并入 locale 并翻译。合并前界面上会显示 key,这是预期的。
- 验收:
  - 阶段条第 4 步不再是占位,统计与表格随队列事件实时更新
  - 失败行能单行重试,批量重试失败项可用
  - 服务商与密钥、并发、重试设置可在此页修改并保存
  - cargo clippy 与 cargo test(concat、concat-drama)通过
- 写入范围:`src/crates/concat-drama/src/checks.rs`, `src/crates/concat/ui/story/generate.slint`, `src/crates/concat/ui/story/review.slint`, `src/crates/concat/src/drama/generate.rs`, `src/crates/concat/src/drama/review.rs`, `src/crates/concat/locales/pending/delivery.json`
- 限制:只改 scope.writes 里的文件;共享文件缺什么改动就 plan block 说明

### 13. 做预览与交付阶段界面(`review-stage-ui`,角色 delivery)

- 依赖:split-story-files, asset-model, provider-settings, queue-controls, delivery-checks
- 目标:按设计稿 Review 画板:左侧 9:16 预览(播放、进度、当前镜头徽标、台词字幕),右侧交付前检查列表和“去剪辑时”三个选项,底部镜头条(可拖动排序、选中、重���成选中的镜头);“导出预览 MP4”按钮。镜头顺序改动写回 story.json。 新增的界面文案不要改 src/crates/concat/locales 下的任何 locale 文件(26 个文件是共享的,并行时会冲突);写进本线路自己的待合并文件 src/crates/concat/locales/pending/delivery.json,格式 {"key":{"en":"...","zh-Hans":"..."}},key 用 story.<区域>.<名字>。最后由 story-locales 任务统一并入 locale 并翻译。合并前界面上会显示 key,这是预期的。
- 验收:
  - 阶段条第 5 步不再是占位,检查列表来自交付前检查逻辑
  - 镜头条拖动排序后重启仍保持
  - 选中镜头可重新生成,新版本作为新 take 不覆盖旧的
  - 导出预览 MP4 能产出文件(Rehearsal 数据下也能跑)
- 写入范围:`src/crates/concat-drama/src/checks.rs`, `src/crates/concat/ui/story/generate.slint`, `src/crates/concat/ui/story/review.slint`, `src/crates/concat/src/drama/generate.rs`, `src/crates/concat/src/drama/review.rs`, `src/crates/concat/locales/pending/delivery.json`
- 限制:只改 scope.writes 里的文件;共享文件缺什么改动就 plan block 说明

### 14. 检查第一波三条线路的衔接(`gate-wave1`,角色 gate)

- 依赖:cast-stage-ui, cast-reference-images, portrait-turnaround-lock, voice-selection, story-agent-tools, director-assistant-pane, delivery-checks, generate-stage-ui, review-stage-ui
- 目标:关卡:第一波(角色与场景、导演助手、生成与交付三条线路)并行做完后,检查它们接口和数据对得上。只验证,不改业务代码。要查:1) 导演助手的 story_* 工具改角色、镜头后,角色与场景阶段和分镜阶段读到同一份数据,且界面同时编辑别的镜头时两边修改都不丢;2) 角色与场景线路产出的声线、定妆图 take 能被生成阶段和交付前检查正确读到;3) 交付前检查对 Rehearsal 产出的数据结论正确;4) 各线路是否只改了自己 scope.writes 里的文件(git diff --stat 对照),有越界的列出来;5) 全部 pending 文案文件格式合法、key 不冲突。用 Rehearsal 假后端和测试即可,不需要真实密钥,不需要启动窗口。done 摘要按三段写:校验了什么、定下的接口与约定、遗留与风险。不通过时 plan fail,由主 Agent 把修复任务交回原线路。
- 验收:
  - cargo test(concat-drama、concat-mcp、concat-api、concat)与 cargo clippy 通过,输出写进证据
  - 跨线路衔接检查(第 1–3 项)各有一条可复现的测试或命令,结果写入摘要
  - 越界改动清单(第 4 项)写明,没有就写“无”
  - 摘要含“校验了什么/定下的接口与约定/遗留与风险”三段

### 15. 实现“去剪辑”:镜头与配音落到时间线(`to-editor`,角色 integrate)

- 依赖:gate-wave1
- 目标:第二波,串行,放在关卡之后因为它要把第一波的成果接进剪���页。点“去剪辑”后切到剪辑页,把选中的 take 作为素材导入,按镜头顺序铺到时间线,配音放独立音轨,台词生成字幕;每个片段带“来自创作页”的标记,记录它对应的镜头 id。先读懂剪辑页现有的导入与时间线接口再做,优先复用,不另起一套。三个选项(自动铺、独立音轨、生成字幕)可单独关闭。 写 done 摘要时引用关卡 gate-wave1 的报告。
- 验收:
  - 时间线上片段顺序与镜头顺序一致,配音在独立音轨(有测试)
  - 字幕内容等于对应台词
  - 片段带来源镜头 id,关闭选项后对应内容不生成
  - cargo clippy 与 cargo test 通过

### 16. 支持从剪辑页回创作页重生成某一镜(`regen-from-editor`,角色 integrate)

- 依赖:to-editor
- 目标:在剪辑页对来自创作页的片段,提供“回创作页重生成”;重生成完成后,新 take 替换到时间线上原来的位置并保持原有的裁剪与特效以外的位置信息。
- 验收:
  - 从片段能跳到创作页对应镜头
  - 新 take 完成后时间线上该片段被替换且位置不变(有测试)
  - 旧 take 仍保留可切回

### 17. 翻译创作页文案到其余 24 种语言(`story-locales`,角色 integrate)

- 依赖:to-editor, regen-from-editor
- 目标:先把 src/crates/concat/locales/pending/ 下 cast.json、assistant.json、delivery.json 以及本波任务新增的文案并入 en.json 和 zh-Hans.json,并删掉 pending 目录;再把创作页相关文案(story.*、titleBar.create/cut 及上面并入的)中现在用英文占位的 24 个语言翻成对应语言,key 保持不变。界面里不应再出现裸 key。
- 验收:
  - 26 个 locale 的 story.* key 集合完全一致
  - 项目里已有的 locale 检查或测试通过

### 18. 在真实窗口里走通创作流程(`walkthrough`,角色 verify)

- 依赖:story-locales, regen-from-editor
- 目标:用 run-concat skill 启动应用,用 Rehearsal 后端从剧本一路走到“去剪辑”:写剧本、拆分镜、建角色并锁定、生成、预览、去剪辑。重点检查剧本与分镜阶段(此前只过了 clippy 和测试,没在窗口里运行过)。把发现的问题逐条记录,不自己修改业务代码。
- 验收:
  - 剧本到去剪辑每一阶段都实际操作过,有截图或日志记录
  - 每个问题写清复现步骤和所在阶段;没有问题就写明“无”
  - 验证完关闭自己启动的应用进程
