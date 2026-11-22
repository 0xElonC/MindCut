# MindCut 剪辑 Agent 调研

## Skill 与工具缺口

**建议先用现有 MCP 做一组原创剪辑 skill，不搬上游提示词，也不等素材库、生成服务或新播放器。** 优先解决短视频的开头、选段、节奏和出片前检查。现有工具能导入本地素材、做非破坏性剪辑、加已有时间码的字幕、逐帧预览并导出；它们还不能让 Agent 直接拿到带词级时间码的转写、静音段、音乐节拍或可搜索的授权素材。[MCP 文档](MCP.md#what-the-agent-can-do) · [MCP 实际目录][tools]

### 对照口径

- **能直接用**：工作流的作用可由 Agent 直接完成，不需要新增 MCP 工具。指能力，不指原文可直接分发。
- **缺工具**：主体流程可映射到现有编辑器，但要完整自动化，需补表中能力。反引号中的缺失工具名是建议接口名，不是已存在的 API。**小**＝现有能力薄封装或简单组合；**中**＝要补分析逻辑或接现有引擎并处理任务状态、校验；**大**＝新渲染/分析管线或第三方服务集成。工作量为相对工程估算，未做原型。
- **需改写**：可以复用思路，但上游依赖另一套状态模型、工具、交互或渲染方式；优先换成 Concat 的做法，不照搬命令。把 `tools.rs::catalogue` 作为实际 MCP 面：`edit_apply` 能原子应用编辑命令，但不是任意后端接口；`captions_add` 只写给定时间段的标题；`clip_frame` 是适配/填满画幅的编辑命令，不是取帧。`preview_frame` 才返回合成帧；选素材时可在独立审片时间线单独放入候选素材后取样，单帧不证明音频或运动效果。应用 UI 已有转写入口（[字幕面板][captions-ui] 调用 [`Transcriber::transcribe`][transcribe]）和[本地语音生成][speech-ui]，不等于 Agent 可调用。模板接口是 `template_list` / `template_instantiate`，实例化要求提供每个槽位的本地文件；不能替代素材搜索或镜头决策。[MCP 实际目录][tools]

### OpenChatCut：全部 30 个 skill

以 [OpenChatCut `src/agent/skills` 的固定快照][occ-root] 为准，表中每行链接到对应 `SKILL.md`。工具对照均基于 [Concat MCP 实际目录][tools] 和 [MCP 文档](MCP.md#what-the-agent-can-do)。

| Skill | 用途 | 分类与 Concat 对照 |
| --- | --- | --- |
| [ai-cinematic-short-film][occ-cinematic] | 从创意做分镜、视觉一致性和逐镜头生成，再装成短片。 | **缺工具**：策划和装配能做；完整生成缺 `image_generate`（大）、`video_generate`（大）。 |
| [asset-import][occ-import] | 按本地文件、公共链接或客户端持有文件选择入库路径。 | **缺工具**：本地走 `media_probe` / `media_import`，不需要其上传会话；URL 路径缺 `media_download`（中，来源白名单、大小/格式、授权记录）。 |
| [batch-montage-variants][occ-batch] | 为同批素材设计多条不同开头、结构或平台版本，并做差异检查。 | **需改写**：用 `project_get`、`edit_apply` 和原生时间线命令（如 `AddTimeline` / `SelectTimeline`），先验一条再批量；版本不是重复导出同一个活动时间线，首版将相似度检查放在选段表中，不自动判重。 |
| [beat-sync-montage][occ-beat] | 按音乐段落强弱和重拍规划镜头，不机械踩每一拍。 | **缺工具**：有时间表就能用 `clips_add_sequence` / `clips_move`；缺 `audio_analyze_beats`（中，返回节拍、强拍及置信度）和 `audio_analyze_sections`（中，返回段落/能量结构）。 |
| [create-motion-graphics][occ-mg] | 编写、放置和修改可编辑的 JSX 动效素材。 | **需改写**：没有任意 JSX 动效资产模型；用 `title_add`、`effects_list`、`effect_layer_add`、`keyframe_set` 和原生模板替代；流程不承诺 JSX 兼容。 |
| [ecommerce-product-montage][occ-ecom] | 按卖点安排商品镜头、演示证据和行动号召。 | **需改写**：用 `preview_frame` 在审片时间线核对素材，`clips_add_sequence`、`title_add` 装配成片；不编造商品事实，也不把别家产品术语做成必经步骤。 |
| [explainer-video][occ-explain] | 把讲解拆成旁白和视觉段落，再对齐到时间线。 | **缺工具**：已有时间码/旁白可手动排轨；完整输入路径缺 `media_transcribe`（中）、`voice_generate`（中，封装已有本地 TTS 及模型状态，若接云另估），自动生成配图另依赖 `image_generate`（大）。 |
| [export][occ-export] | 导出成片、跟进渲染并向用户交付文件。 | **需改写**：常规成片走 `project_save` → `export_start` → `export_wait`，按需要用 `export_status` / `export_cancel`；不继承其跨设备取件、动效 ProRes 和 FCPXML 流程或承诺。 |
| [image-gen][occ-image] | 选模型生成图像或编辑参考图，并追踪结果。 | **缺工具**：`media_import` 只能接落地文件；缺 `image_generate`（大，服务、授权、费用确认和异步作业）。 |
| [known-errors][occ-errors] | 按已知错误处理上传、编解码、编辑和重试问题。 | **需改写**：改用 Concat 实际错误、`concat_version`、`media_probe`、`timeline_read` 和 `export_status`；不要照搬上游重试条件。 |
| [livestream-to-clips][occ-live] | 找出直播高光、挑选不同片型，并检查批量出片。 | **缺工具**：剪辑、标题和导出已有；缺 `media_transcribe`（中）和 `media_find_highlights`（大，长素材音视频联合候选及置信度）；有用户标注时可先人工选段。 |
| [long-video-to-shorts][occ-long] | 从一个长源选能独立成立的高光，剪成竖版短片。 | **缺工具**：已有带时间码的逐字稿时，`clip_trim` / `clips_split`、`project_set_video`、`clip_transform` / `clip_update`、`captions_add` 已够做人工构图版；缺 `media_transcribe`（中），自动跟人构图另缺 `media_analyze_subject_track`（大，可后置）。 |
| [motion-graphic-placement][occ-place] | 先定动效的出现时间、占位以及和内容的关系。 | **需改写**：保留时机和避挡原则；用 `title_add`、`clip_transform`、`keyframe_set` 和 `preview_frame` 放置原生标题/效果，不依赖 JSX 资产。 |
| [multi-clips-to-reels][occ-multi] | 从多个已有素材筛选镜头，剪成一条有主题的短片。 | **需改写**：`timeline_read` / `project_get` 查库存，`preview_frame` 在独立审片时间线取样，`clips_add_sequence` 组接；不沿用上游的素材帧和脚本接口。 |
| [music-intelligence][occ-music-intel] | 为卡点剪辑提取节拍、段落和音乐编辑建议。 | **缺工具**：已有节拍表可按秒批量剪；缺 `audio_analyze_beats`（中）和 `audio_analyze_sections`（中）。优先返回分析结果，先不新增自动改时间线接口。 |
| [music][occ-music] | 按画面、风格和时长生成配乐，并跟进异步作业。 | **缺工具**：本地配乐可导入、用 `clip_update` 调音量与淡入淡出；缺 `music_generate`（大，生成服务、费用与权利记录）。 |
| [news-rough-cut][occ-news] | 用采访和旁白转写对齐新闻叙事，组不杜撰引语的粗剪。 | **缺工具**：已有精确时间码可用 `clips_add_sequence`、`track_add`、`preview_frame`；缺 `media_transcribe`（中）供内部检索与核对引语。 |
| [openchatcut-plugin-basics][occ-basics] | 说明项目寻址、素材、时间线、预览和导出的工具顺序。 | **需改写**：换成 `concat_version`、`project_*`、`timeline_read` 和 `edit_apply`；项目以目录 `path` 寻址，不能复用上游项目 ID、会话或编辑链接。 |
| [product-ad-video-script][occ-ad] | 从商品卖点、受众和已有证据写短广告脚本与分镜提纲。 | **能直接用**：纯规划，不写时间线；有项目时可用 `project_get` 补素材背景。 |
| [product-help][occ-help] | 解释上游编辑器的界面、能力、服务商和密钥配置。 | **需改写**：只引用 Concat 实际文档与 `concat_version` / `effects_list`；上游配置及生成能力不适用。 |
| [shader-gen][occ-shader] | 创建、修订并把自定义 shader 用作效果或转场。 | **需改写**：当前 `effects_list`、`clip_update`、`effect_layer_add` 只组合可用效果/转场，不提供写入和运行任意 shader 的接口。 |
| [skill-creator][occ-creator] | 创建、修订和验证 OpenChatCut 内可安装的工作流。 | **需改写**：把 `load_skill` / `manage_skill` 改成 Codex 文件式发现和显式选择（见下文）；不为业务 Agent 开全局安装权限。 |
| [storyboard-shot-breakdown][occ-storyboard] | 把脚本拆成带叙事目的、时长和画面动作的镜头表。 | **能直接用**：纯规划可输出镜头清单；若要落时间线，则另用已有媒体和 `clips_add_sequence`。 |
| [talking-head-guide][occ-talk] | 口播去废话、整句精剪、上字幕、铺辅助镜头和包装。 | **缺工具**：已知切点可用 `clips_split` / `clips_remove` / `clips_move`，已知时间码可用 `captions_add` / `title_style_words`；缺 `media_transcribe`（中）、`audio_detect_silence`（中，需对噪声和停顿保护做校验），自动找辅助镜头另缺 `stock_media_search`（中）和 `media_download`（中）。 |
| [transcription][occ-transcribe] | 跟踪和修复转写，再搜索词级时间供字幕或剪辑使用。 | **缺工具**：`captions_add` 不是 ASR；建议 `media_transcribe`（中，复用本地模型，提供段/词时间码和失败原因）与 `media_transcript_get`（小，读取/搜索已缓存转写；首次重试与状态可由前者承载）。 |
| [verification][occ-verify] | 同时看工程结构和合成帧，不能用工具成功代替画面证据。 | **需改写**：用 `timeline_read` / `project_get` 和 `preview_frame` 分点验证；首版不依赖联系表或上游追踪日志；音频/运动须明确未由静帧检查。 |
| [video-gen][occ-video] | 从提示词和参考素材生成视频，等待结果后落时间线。 | **缺工具**：已有生成文件可 `media_import` / `clip_add`；缺 `video_generate`（大，运行时长、费用、授权、异步失败归因）。 |
| [video-thumbnail-generator][occ-thumb] | 用真实视频帧做封面，避免虚构人物、产品或界面。 | **需改写**：用 `preview_frame` 在审片时间线选取真实画面，用 `title_add` 排版，然后输出对应的合成帧 PNG；不照搬生成模型分支。该快照的目录里未找到正文所列三个缩略图参考文件，未核实其内容。 |
| [voice][occ-voice] | 选声试听、生成旁白或定制音效，并在剪辑后对齐。 | **缺工具**：对已有录音可 `media_import` / `clip_add` 并人工对齐；缺 `voice_generate`（中，本地 TTS）、`voice_preview`（小，封装短试听）、`sound_generate`（大，定制音效管线与授权）。 |
| [widget-forms][occ-forms] | 在聊天中以可选择的表单收集结构化用户决策。 | **需改写**：上游 `ask_followup_questions` 是编辑器交互，不是剪辑动作；首版用普通文字询问必要事项，不自造表单接口或替用户选。 |

上述时间线命令见 [`Command` 定义][timeline-commands]；它们未作为独立 MCP 工具发布。工具表未把 UI 中存在但 MCP 未暴露的分析、TTS 或界面行为算成可直接调用。

### MakeMyClip 与 Awesome 清单：对短视频最有用的补充

从 Awesome 快照的 [Editing][awesome-editing]、[Shorts & social][awesome-shorts] 及工具条目筛选；不采用目录的安全徽章作为授权或运行依据。MakeMyClip 的仓库只有一个 `SKILL.md`；下表按完整工作流只算一个候选。[MakeMyClip README][mmc-readme]

| Skill | 用途 | 分类与 Concat 对照 |
| --- | --- | --- |
| [MakeMyClip `editor`][mmc] | 本地口播精剪：看逐字稿去停顿/重录，拆可独立成篇的竖版短片，再上字幕和规范响度。 | **缺工具**：`media_probe`、`clips_split`、`clips_remove`、`clip_transform`、`captions_add` 和 `export_*` 可接管基础流程；缺 `media_transcribe`（中）、`audio_detect_silence`（中）、`audio_normalize_loudness`（中，测量并校验目标响度；`clip_update` 的音量不是响度标准化）、`export_editable_project`（大，仅跨编辑器交付支线）。不执行内置 `npx` 或导出再导入替代原生时间线。 |
| [AgriciDaniel `shorts`][claude-shorts] | 读取逐字稿后先推荐并确认候选，再把边界贴近语句/停顿，做竖屏字幕与导出。 | **缺工具**：`media_transcribe`（中）、`audio_detect_silence`（中）；用 `project_set_video`、`clip_transform`、`captions_add` / `title_style_words` 和 `export_*` 代替外部渲染，不保证逐字动画等价。 |
| [louisedesadeleer `cut-video`][cut-video] | 粗精剪间先产生候选切点，用强制对齐和音频证据保护笑声、自然停顿及词头词尾。 | **缺工具**：缺 `media_transcribe`（中）、`audio_detect_silence`（中）、`audio_align_words`（大，音文强制对齐与多语言验证）；现有 `edit_apply` 只执行已确认的无损切点计划，不运行其环境安装指令。 |

### Codex 怎么发现，内置 skill 怎么分发

1. **当前不是应用自己扫描。** [`concat/src/agent.rs::load_skills`][load-skills] 把活动项目目录（无项目回退到用户家目录，最后到 `.`）传给 Codex 的 `skills/list`；[`wire.rs::skills_params`][wire-skills] 发 `cwds: [cwd]`、`forceReload: false`。[`wire.rs::read_skills`][wire-skills] 读取返回的 `data[].skills[]`，去掉显式禁用项、要求 `name` / `path`，按名称排序并按精确同名去重；这不是管理同名冲突的优先级机制。`agent.rs::setup` 也把同一项目目录给线程；显式选择的 skill 由 [`agent.rs::send`][send-skill] 和 [`wire.rs::turn_params`][turn-skill] 传 `type: "skill"`、名称及 `SKILL.md` 路径。应用寻找并启动本机 `codex app-server`，不是自己的 skill 服务，也没有给子进程设置专用 `CODEX_HOME`。[二进制定位][locate] · [启动代码][rpc]
2. **发现位置。** [OpenAI Codex 文档][codex-skills] 列明项目目录 `.agents/skills/`（在 Git 仓库中也扫从当前目录到仓库根的上级 `.agents/skills/`）、全局 `$HOME/.agents/skills/`、管理员 `/etc/codex/skills/` 和 Codex 自带的系统 skill。可以用目录软链接，但不能假定应用安装包 `Resources`、E-Flow 自己的 skill 路径或 `.claude/skills` 会被 Codex 自动扫描。手动验证可把原创 `eloncut-*` skill 目录放在项目 `.agents/skills/`（仅该项目）或 `$HOME/.agents/skills/`（所有项目），通过 Agent 选择器/`skills/list` 看实际回包。
3. **建议的内置分发，尚未实现。** 源码随应用维护 `src/crates/concat/agent-skills/eloncut-*/SKILL.md` 和相对引用文件。在 [`concat/build.rs::main`][build] 仿现有文字预设嵌入资源的做法，把整个文本目录编入桌面二进制；不依赖各 OS 的包路径。首次启动 Agent 时给用户一个明确的“安装内置剪辑技能”选项，将完整目录以真实文件写入 `$HOME/.agents/skills/eloncut-*`，按自有清单更新，保留用户修改和同名第三方目录，不写每个项目、不假装管理员安装。只需装一次就能让同一用户的每个项目被发现，代价是外部 Codex 也会看到；说明这个范围并提供卸载。未同意全局安装时才提供按项目安装。初装/更新后重新列技能（必要时强制刷新或重启 app-server），用两个不同目录验证发现、启用状态和显式调用，升级覆盖和 Windows 用户目录也需验证。现有仓库只见 `.claude/skills/run-concat/SKILL.md` 开发用技能；未发现 Concat 分发的剪辑 skill 或专门安装逻辑。这是源码核对，不是已经跑过安装包。[现有构建资源][build] · [应用打包][packaging]

### 许可证：不把 OpenChatCut 改写版直接放进双授权发行物

- [Concat `LICENSING.md` 第 3–26 行](../LICENSING.md#L3-L26) 定义 `AGPL-3.0-or-later` 与业主签署的商业许可；[第 36–52 行](../LICENSING.md#L36-L52) 说明第三方保留自己的许可、不被这两种许可覆盖，且自有贡献按 MIT No Attribution 的签署流程进入。OpenChatCut [根 `LICENSE`][occ-license] 是 GNU AGPL v3；[skill `NOTICE.md` 第 3–10、12–18 行][occ-notice] 的特定声明说这些文件改自 `ChatCut-Inc/agent-plugin`，并写明原插件标识为 `GPL-3.0-only`，NOTICE 也涵盖配套文件。不能用根许可证覆盖这段来源信息。
- **原插件后来确有改许可，但不能外推到 OpenChatCut。** 原插件[旧根许可证][chatcut-old] 是 GNU GPL v3；其 [2026-10-30 的改许可提交][chatcut-change] 把 agent plugins 改为 Apache-2.0，[原插件当前核对快照根许可证][chatcut-now] 也为 Apache-2.0。还未确认 OpenChatCut 每份文字来自哪个原版修订、改许可是否覆盖具体要复制的文字、以及谁能授权 OpenChatCut 后续新增表达。若考虑直接取原插件 Apache 版本，必须逐文件核版、确认权利链并遵守 [Apache 2.0 §4][apache-terms]，不是把 OpenChatCut 的版本换个标签。
- **开源版和商业版分开判断。** [GNU AGPL v3 §13][agpl-13] 与 [GNU GPL v3 §13][gpl-13] 允许符合条件的 GPLv3 与 AGPLv3 组合并传播，不是把 GPL 部分换成 AGPL 或闭源许可；还涉及保留声明和提供相应源文件（见各自 §4–6、§13）。因此开源发行并非当然不能含 GPL 内容，但是否可复制这份具体文本、适用哪一版授权、怎样履行义务，**需要律师或版权方确认**。双授权的商业一侧不能靠 Concat 的书面许可为第三方文字重授闭源使用权。GPL 素材是否可以独立、仅并列分发而不影响其余商业组件，是依组合方式、发行物和实际商业协议而定的问题，**需要律师或版权方确认**。在确认前，不复制、翻译或近似改写这些文字与参考文件，也不让它们进入商业安装包；GPL 不等于禁止收费，这里的风险是不能承诺按 Concat 自有商业条款交付第三方内容。
- **替代方案。** 首选只借“先选段、再剪、最后验帧”等工作思路，对照 Concat 工具及自有素材从零写文本、步骤示例和测试样片，并保留来源与原创记录。也可单独评估有 [MakeMyClip MIT 正文][mmc-license] 的文件：MIT 允许商业再分发，但有保留原版权/许可声明的条件，且不能把其运行时、模型或媒体的权利一并视为已获得；接入前仍需核相关文件与依赖。若一定要保留 OpenChatCut 文案，请版权方针对拟分发的具体文件与版本提供明确书面许可。本调研不作法律意见，也不对英文条款未确认部分给出确定结论。

### 推荐先做（按顺序）

1. **短片验收**（参考 [verification][occ-verify]、[Concat MCP][tools]）：所有先做 skill 共用。检查工程结构＋切入/字幕/结尾的合成帧，报告静帧看不出的内容；导出只在明确需要时进行。
2. **多素材转竖版短片**（参考 [multi-clips-to-reels][occ-multi]、[ecommerce-product-montage][occ-ecom]）：用本地素材、开场钩子、证据镜头和结尾重新组接；不依赖新工具，最容易看出节奏和叙事的改善。
3. **长素材转短片**（参考 [long-video-to-shorts][occ-long] 和 [shorts][claude-shorts]）：先支持用户给时间码或逐字稿，选能独立成立的整段，检查竖屏构图；再补本地转写，不等自动跟人裁切。
4. **口播精剪与字幕**（参考 [talking-head-guide][occ-talk]、[MakeMyClip][mmc]、[cut-video][cut-video]）：先把已确认时间码做成可撤销的剪辑与字幕，加入转写和静音候选后再自动化；保留语气、笑声和自然停顿，不无条件删静音。
5. **音乐卡点混剪**（参考 [beat-sync-montage][occ-beat]、[music-intelligence][occ-music-intel]）：先允许用户给节拍/音乐段落，在重拍上落关键镜头；后续接节拍分析，不把每一拍都剪成同样的长度。

**核查边界：** 读取上游固定快照、官方页面和本地源码，没有安装或执行任何外部 skill，没有跑 Codex/app-server、渲染、调用素材服务或验证许可证覆盖范围。表中的工作量和分发步骤是方案，不是已实现功能。

[tools]: ../src/crates/concat-mcp/src/tools.rs#L165
[captions-ui]: ../src/crates/concat/src/panes/captions.rs#L634
[transcribe]: ../src/crates/concat-speech/src/transcribe.rs#L1074
[speech-ui]: ../src/crates/concat/src/panes/speech.rs#L4
[timeline-commands]: ../src/crates/concat-project/src/commands/mod.rs#L907
[load-skills]: ../src/crates/concat/src/agent.rs#L829
[wire-skills]: ../src/crates/concat-agent/src/wire.rs#L130
[send-skill]: ../src/crates/concat/src/agent.rs#L895
[turn-skill]: ../src/crates/concat-agent/src/wire.rs#L93
[locate]: ../src/crates/concat-agent/src/locate.rs#L28
[rpc]: ../src/crates/concat-agent/src/rpc.rs#L204
[build]: ../src/crates/concat/build.rs#L40
[packaging]: ../.github/workflows/build-app.yml#L529
[codex-skills]: https://developers.openai.com/codex/skills/#where-to-save-skills
[occ-root]: https://github.com/0xsline/OpenChatCut/tree/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills
[occ-license]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/LICENSE
[occ-notice]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/NOTICE.md#L3-L18
[occ-cinematic]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/ai-cinematic-short-film/SKILL.md
[occ-import]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/asset-import/SKILL.md
[occ-batch]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/batch-montage-variants/SKILL.md
[occ-beat]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/beat-sync-montage/SKILL.md
[occ-mg]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/create-motion-graphics/SKILL.md
[occ-ecom]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/ecommerce-product-montage/SKILL.md
[occ-explain]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/explainer-video/SKILL.md
[occ-export]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/export/SKILL.md
[occ-image]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/image-gen/SKILL.md
[occ-errors]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/known-errors/SKILL.md
[occ-live]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/livestream-to-clips/SKILL.md
[occ-long]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/long-video-to-shorts/SKILL.md
[occ-place]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/motion-graphic-placement/SKILL.md
[occ-multi]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/multi-clips-to-reels/SKILL.md
[occ-music-intel]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/music-intelligence/SKILL.md
[occ-music]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/music/SKILL.md
[occ-news]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/news-rough-cut/SKILL.md
[occ-basics]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/openchatcut-plugin-basics/SKILL.md
[occ-ad]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/product-ad-video-script/SKILL.md
[occ-help]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/product-help/SKILL.md
[occ-shader]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/shader-gen/SKILL.md
[occ-creator]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/skill-creator/SKILL.md
[occ-storyboard]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/storyboard-shot-breakdown/SKILL.md
[occ-talk]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/talking-head-guide/SKILL.md
[occ-transcribe]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/transcription/SKILL.md
[occ-verify]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/verification/SKILL.md
[occ-video]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/video-gen/SKILL.md
[occ-thumb]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/video-thumbnail-generator/SKILL.md
[occ-voice]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/voice/SKILL.md
[occ-forms]: https://github.com/0xsline/OpenChatCut/blob/f96502a18e65a09c19324b10feb671bb505056bd/src/agent/skills/widget-forms/SKILL.md
[mmc]: https://github.com/MakeMyClip/editor/blob/e090b420f2f0c2c5fd4b16d3b741348fdd6d8671/SKILL.md
[mmc-readme]: https://github.com/MakeMyClip/editor/blob/e090b420f2f0c2c5fd4b16d3b741348fdd6d8671/README.md
[mmc-license]: https://github.com/MakeMyClip/editor/blob/e090b420f2f0c2c5fd4b16d3b741348fdd6d8671/LICENSE
[awesome-editing]: https://github.com/zhuyansen/awesome-claude-video-skills/blob/4a0a0f7220800f1e95e4a326a4b88accb33c5c4f/README.md#L441-L517
[awesome-shorts]: https://github.com/zhuyansen/awesome-claude-video-skills/blob/4a0a0f7220800f1e95e4a326a4b88accb33c5c4f/README.md#L518-L601
[claude-shorts]: https://github.com/AgriciDaniel/claude-shorts/blob/a369fad9d84287edbb8fa3ab515f268e770f683b/SKILL.md
[cut-video]: https://github.com/louisedesadeleer/cut-video/blob/a5a87a64f381c62b8972a92132fb48015b0e50ff/SKILL.md
[chatcut-old]: https://github.com/ChatCut-Inc/agent-plugin/blob/e4e348a54f73dfeddc78ab27a7e5ed1602d85918/LICENSE
[chatcut-change]: https://github.com/ChatCut-Inc/agent-plugin/commit/f5afda21127e2ba86c1ba1851dae0e5bf62529a3
[chatcut-now]: https://github.com/ChatCut-Inc/agent-plugin/blob/a69a54fac266c69a9e76d2767af2e82776a17464/LICENSE
[apache-terms]: https://www.apache.org/licenses/LICENSE-2.0#redistribution
[agpl-13]: https://www.gnu.org/licenses/agpl-3.0.en.html#section13
[gpl-13]: https://www.gnu.org/licenses/gpl-3.0.en.html#section13

## 素材来源

**推荐先支持用户自己的文件；外部素材只做“用户发起搜索、确认一条、下载进当前工程”。** 视觉素材优先评估 Coverr，Pexels 留作备选；内置声音库单独用可再分发的 CC0 音源。能用在商用成片、能通过 API 展示、能打包原文件分发，是三件不同的事。[Coverr 许可][coverr-license] · [Pexels API 条款 FAQ][pexels-terms] · [现有声音库规则](AUDIO_LIBRARY.md#rules-for-the-selection)

### 四家视觉素材站

| 来源 | 官方 API | 密钥 | 限流 | 授权、商用与署名 | 禁止事项及接入条件 |
| --- | --- | --- | --- | --- | --- |
| Pexels | 有，官方帮助中心确认图片和视频 API。[API 说明][pexels-api] | 需要账户的 API key；官方帮助中心核查时称**新密钥发放暂停**，没有现成合规凭证就不能作为首发依赖。[密钥 FAQ][pexels-key] | 默认每小时 **200**、每月 **20,000**；核查时提高限额也暂停。[限额 FAQ][pexels-rate] | 官方说明网站、应用、插件和 API 共用基本条款；普通照片/视频允许商用且成片通常不必署名，但 API 集成的额度审核要求平台标明 Pexels 和贡献者；建议在搜索结果始终提供来源和作者链接，不把成片免署名等同于平台无需署名，产品接入前复查完整 API 展示条件。[API 条款 FAQ][pexels-terms] · [许可 FAQ][pexels-license] · [商用 FAQ][pexels-commercial] · [额度审核要求][pexels-rate] | 禁止单独转售或分发没有实质创意改动的原素材、搭建同类素材库、未经许可做批量/系统性获取，或收集媒体及元数据做模型训练、微调、评估等。被摄人物、品牌和商标的权利仍需单独核对。[API 条款 FAQ][pexels-terms] · [商用 FAQ][pexels-commercial] |
| Pixabay | 有，提供图片和视频 REST 搜索，视频端点为 `GET /api/videos/`。[官方 API 文档][pixabay-api] | `key` 必填，从登录后的 API 文档取得。避免把查询串密钥写到错误日志。[视频搜索文档][pixabay-videos] | 默认每个 key **100 请求／60 秒**，不是按 IP；请求结果必须缓存 **24 小时**，可读 `X-RateLimit-*`，超过会返回 HTTP 429。[官方限流][pixabay-rate] | API 文档称取得的图片/视频用与网站相同的 **Pixabay Content License**，且要求在 API 搜索结果展示 Pixabay 来源。**未核实**：网页 Content License 的商用细则、成片的署名义务及人物/商标限制，官网许可摘要与完整条款实测 HTTP 403；不要仅凭 API 文档的 royalty-free 推定。API 文档本身经网页转译入口读到原文。[官方 API 文档][pixabay-api] · [网页许可摘要][pixabay-license] · [完整条款][pixabay-terms] | API 要求真实用户搜索，不允许系统性批量下载或大量自动查询；图像 URL 可用于临时结果，但不能长期热链；视频可嵌入，官方仍建议自行保存。网页条款里关于原素材再分发、品牌等的其他限制**未核实**（同上）。上线前再审查是否允许 Agent 搜索方式及缓存边界。[API 限流与热链][pixabay-rate] · [完整条款][pixabay-terms] |
| Coverr | 有，官方开发者页支持 REST，也提供 MCP；首版若接入只用 REST 搜索与下载，不暴露生成端点。[开发者页][coverr-dev] | 需要在 Profile → API 建应用并取得 key；用官方建议的 `x-api-key` 请求头，不把 key 拼入日志或 Agent 消息。[认证文档][coverr-auth] | Demo **50 次／小时**；Production **2,000 次／小时**，官方 FAQ 说明无月度调用上限；下载额度另有档位，Enterprise 可申请自定义，未核实具体下载额度数字。[开发者页与配额 FAQ][coverr-dev] | 最新许可页明写 API 与网页遵循相同素材规则，按 stock / AI 内容区分；可做商用编辑产品，网页下载不强制署名，Demo 集成要标 Coverr 和列明的作者，Production 不要求。开发者 FAQ 与旧文档的署名／商业措辞更笼统，选商用上线档位前应请 Coverr 确认，以新版明确许可为核查起点；Premium 的计划门槛、预览水印及赞助品另算。[许可：Using the API][coverr-license] · [开发者 FAQ][coverr-dev] · [旧开发说明][coverr-legacy] | 不原样卖／重传素材、不做同类素材库、不批量下载；禁训练、微调、蒸馏、评估或制作这类数据集（包括 API 元数据）。用户实际选择／下载视频时要调用 `PATCH /videos/:id/stats/downloads`，仅预览不报；品牌、人物、地点权利不能由素材许可替代。建议默认过滤 `is_ai_generated:false`，不将素材送去训练或评估。[许可及下载统计][coverr-license] |
| Mixkit | **未找到公开官方 API**：官方条款、许可和视频页只确认网页使用，站点 `/api/` 返回 HTTP 404；不能据此排除私有合作接口。[网页资源][mixkit-videos] · [使用条款][mixkit-terms] · [探测地址][mixkit-api] | **未核实**；未见公开发钥说明，网页手动下载不代表有匿名 API。[网页资源][mixkit-videos] · [使用条款][mixkit-terms] | **未核实**；没有公开 API 配额声明，不能把网页下载宣传视为自动访问限额。[网站条款][mixkit-terms] · [探测地址][mixkit-api] | **API 与网页是否同许可未核实**。手动下载必须看每项标签：Stock Video Free 可用于商用／非商用项目，Restricted 只能个人使用；视频 FAQ 说不强制署名，但不能把它套到其他类别或赞助素材。[官方许可清单][mixkit-license] · [视频 FAQ][mixkit-videos] | 禁未实质加工就转卖、汇编成竞争素材库、用 bot／脚本批量下载；不要抓站点来当 API。只建议用户自行挑选 Free 项，拿已获许可的本地文件导入。[使用条款][mixkit-terms] · [官方许可清单][mixkit-license] |

所谓“已核实”指读到公开资料，不代表已开通密钥、获得商用集成批准或试过下载。拟接外站时，搜索与写入项目分开；记录站点、素材 ID、原页、作者、许可与核查日期，展示水印/商用/AI 标记；没有符合许可的结果就建议本地替代，不自动换站、不批量缓存原始媒体。搜索词应仅包含用户同意发给服务商的必要内容，不发送工程名、原始时间线或逐字稿。[Coverr 许可][coverr-license] · [Pexels API 条款 FAQ][pexels-terms] · [Pixabay API 文档][pixabay-api] · [Mixkit 条款][mixkit-terms]

### 音乐、音效与现有声音库

- **内置可再分发候选：** 优先手工策展 [Freesound 的 CC0 音效][freesound-faq] 和 [OpenGameArt 的逐条 CC0 音乐][oga-cave]（另一个已见许可标识的示例是 [Another Space Background Track][oga-space]），适合先备转场、提示、环境声和少量音乐床。不能把 Freesound 的其他许可或 OpenGameArt 的整个合集视为 CC0；逐条存作者和原始页面，核版权与可识别内容。Freesound API 自身另有非商用／商用规则，代码接入商用前需取得相应许可，不影响手工核验素材；[Kenney Interface Sounds][kenney-interface] 可作后备，但本次官网返回 HTTP 403，具体许可**未核实**。[Freesound 开发者页][freesound-dev] · [CC0 正文][cc0-terms]
- **成片配乐而非内置库：** [Mixkit 音乐 FAQ][mixkit-music] 允许用在 YouTube、播客等适用场景，通常不要求署名，但不适用于游戏、光盘、电视／广播等 FAQ 列明的场景，也不要把原始音轨塞进可下载的音效库。[Mixkit 音效页][mixkit-sfx] 和 [Coverr Stock Library][coverr-license] 可供用户选用，仍按具体项目、文件和分发方式核对。现有 Concat 库要求 CC0，不录入这些只有自定义使用许可的素材。[AUDIO_LIBRARY.md](AUDIO_LIBRARY.md#rules-for-the-selection)
- **现状：代码有货架，公开仓没音频。** [AUDIO_LIBRARY.md](AUDIO_LIBRARY.md) 和 [`models.rs::SOUNDS_MANIFEST` / `sound_sources`][sound-config] 表明：点击加载才拉 `concatapp/assets` 的 `audio/manifest.json`，播放/使用时先走 `audio-v1` release、失败再走仓库文件；不随应用发放声音。货架过滤非 CC0、非法文件名和无有效摘要的记录，下载校验 SHA-256，使用时复制到工程 `audio/`，不依赖缓存路径。[`sounds.rs` 清单与缓存实现][sound-store]。公开仓库本身存在；[快照目录树][asset-tree] 有 `effects/`、`filters/`、`templates/`，没有 `audio/`；[清单 URL][sound-manifest] 和 [镜像 release tag][sound-release] 均实测 HTTP 404。因此当前首次加载没有公开声音清单可用，走“Sound library coming soon”分支；这不等于用户自己添加的音频不可用。

## 模板

**有本地工程模板引擎，现有内置工程模板数为 0。** 它与已内置的特效、文字样式是三类不同资产。`template_list` 只列配置目录里已保存／安装的有效模板；未发现打包或首次启动预置工程模板的路径。[API 分派][template-dispatch] · [模板列表实现][template-list] · [构建资源][build] · [应用打包][packaging]。公开 `ConcatApp/assets` 的 [`templates/README.MD`][asset-templates] 只有 `todo`，没有可下载的工程模板；也没有看到 `template_list` 读取它。

| 项目 | 源码核对结果 |
| --- | --- |
| API 路径 | [`tools.rs::catalogue`][template-mcp] 把 `template_list` / `template_instantiate` 转到 `template.list` / `template.instantiate`；[`concat-api/src/lib.rs`][template-dispatch] 调 `templates::list(&self.dirs.config)`，实例化则先检查目标位置及 probe 每个 `{mediaId, file}`，再由 [`concat-host/src/templates.rs::instantiate`][template-instantiate] 建工程并打开。宿主还有 `template.save` 路由，当前 MCP 目录没有模板保存工具。[API `instantiate`][template-engine-api] |
| 格式 | 一个目录一个工程模板，不是视频文件或单条特效：`template.json` 是编辑器原生工程文档（含 `video.width`、`height`、`rateNum`、`rateDen`、媒体和时间线）；`assets/` 放模板自带的音乐、图层、字体等，文档用 `assets/...` 相对路径；可选 `poster.jpg` 作卡片。`save` 用现有工程写法打包、清空占位素材路径、不覆盖同名模板；实例化复制内置素材进新工程，更新路径并应用槽位替换。[格式及 `save`][template-format] · [实例化][template-instantiate] |
| 位置 | `<AppDirs.config>/templates/<模板目录>/`。桌面默认：macOS `~/Library/Application Support/app.concat.editor/templates/`；Windows `%APPDATA%/app.concat.editor/templates/`；Linux `$XDG_CONFIG_HOME/app.concat.editor/templates/`（未设置时 `~/.config/app.concat.editor/templates/`）；若可执行文件旁已启用 `portable/`，则为其下的 `templates/`。不要把声音缓存目录、社区仓库或源码路径当成模板库。[`templates_dir`][template-dir] · [`AppDirs::locate`][app-dirs] · [`apple` / `portable_root`][apple-dirs] |
| 槽位 | 文档 `media[]` 中 `placeholder: true` 的素材就是槽位，`mediaId` 是媒体 ID（不是片段 ID）；模板素材的占位路径为空。`slots_of` 显示 `mediaId`、`name`、`kind`、`seconds`，按活跃时间线首次用到排序，`seconds` 为该线引用片段的时长之和，不是所需源文件时长。每个槽位必须填，同一个槽位多次复用只填一次；`FillSlot` 在所有时间线维持引用 ID、保留时间线起点／时长／速度，源入点归零并更新媒体与音轨信息。视频过短可能末帧定格，不能假设自动补齐叙事或智能裁切；应按模板建议对时长、画幅和种类做预检。[`slots_of`][template-slots] · [实例化][template-instantiate] · [`Command::FillSlot`][fill-slot] |

### 现有资产与可借用内容

- **本地盘点：** [`concat-effects/packages`][effects-packages] 的 `effect.toml` 共 **132** 份：调色滤镜 **57**、画面效果 **28**、转场 **22**、音频处理 **25**；这是该目录清册，不是运行时所有平台的可用数，也不含字幕。例：效果库有抖动、光晕、淡入淡出和语音增强。另在 [`concat/text-presets`][text-presets] 有 **45** 份 `preset.toml`，`build.rs` 编入程序，由 `presets.rs::builtin` 读取；包括 Bold caption、下三分之一标题、通用标题和多语种字体样式，不是 45 套字幕动画。MCP `captions_add` 可按时间码给统一样式，`title_style_words` 可做部分词强调，不等于可直接调用整套预设卡片。[构建][build] · [`presets.rs::builtin`][builtin-text] · [MCP 文档](MCP.md#what-the-agent-can-do)
- **可借鉴字幕样式：** [AgriciDaniel/claude-shorts 的样式参考][shorts-styles] 和 [源码组件][shorts-components] 有 Bold／Bounce／Clean，固定快照 [根许可证][shorts-license] 为 MIT，复制覆盖的代码要保留版权和许可声明。React／Remotion 代码不是 Concat 模板；建议只抽取视觉规则（词组分行、当前词强调、安全区），用现有标题、字词样式和关键帧制作原生效果，不捆绑其运行时或字体。若后续要引入任何依赖或样例媒体，分别核其授权。
- **成套模板：** 未找到可直接载入 Concat 的、许可证明确的第三方工程模板；Mixkit 有其他编辑器的模板，但属于 [Mixkit 独立模板条款][mixkit-license]，不是可打包重分发的开源 Concat 模板。第一批用自有文本、图形和可核验许可的字体/声音制作，再用原生 `template.save` 生成，不迁移外部工程格式。[模板保存逻辑][template-format] · [公开模板目录][asset-templates]

[pexels-api]: https://help.pexels.com/api/v2/help_center/en-us/articles/900005304306.json
[pexels-key]: https://help.pexels.com/api/v2/help_center/en-us/articles/900004904026.json
[pexels-rate]: https://help.pexels.com/api/v2/help_center/en-us/articles/900005852323.json
[pexels-terms]: https://help.pexels.com/api/v2/help_center/en-us/articles/900005880463.json
[pexels-license]: https://help.pexels.com/api/v2/help_center/en-us/articles/360042295174.json
[pexels-commercial]: https://help.pexels.com/api/v2/help_center/en-us/articles/360042295214.json
[pixabay-api]: https://pixabay.com/api/docs/
[pixabay-videos]: https://pixabay.com/api/docs/#api_search_videos
[pixabay-rate]: https://pixabay.com/api/docs/#api_rate_limit
[pixabay-license]: https://pixabay.com/service/license-summary/
[pixabay-terms]: https://pixabay.com/service/terms/
[coverr-dev]: https://coverr.co/developers
[coverr-auth]: https://api.coverr.co/docs/auth/
[coverr-license]: https://coverr.co/license#using-the-api
[coverr-legacy]: https://api.coverr.co/docs
[mixkit-videos]: https://mixkit.co/free-stock-video/
[mixkit-music]: https://mixkit.co/free-stock-music/
[mixkit-sfx]: https://mixkit.co/free-sound-effects/
[mixkit-license]: https://mixkit.co/license/
[mixkit-terms]: https://mixkit.co/terms/
[mixkit-api]: https://mixkit.co/api/
[freesound-faq]: https://freesound.org/help/faq/
[freesound-dev]: https://freesound.org/help/developers/
[oga-cave]: https://opengameart.org/content/cave-explorer
[oga-space]: https://opengameart.org/content/another-space-background-track
[kenney-interface]: https://kenney.nl/assets/interface-sounds
[cc0-terms]: https://creativecommons.org/publicdomain/zero/1.0/
[sound-config]: ../src/crates/concat-host/src/models.rs#L49
[sound-store]: ../src/crates/concat-host/src/sounds.rs#L288
[asset-tree]: https://github.com/ConcatApp/assets/tree/0f6274a0c526ec459ed8aaa90075cd47f12411e0
[sound-manifest]: https://raw.githubusercontent.com/concatapp/assets/main/audio/manifest.json
[sound-release]: https://api.github.com/repos/concatapp/assets/releases/tags/audio-v1
[asset-templates]: https://github.com/ConcatApp/assets/tree/0f6274a0c526ec459ed8aaa90075cd47f12411e0/templates
[template-mcp]: ../src/crates/concat-mcp/src/tools.rs#L439
[template-dispatch]: ../src/crates/concat-api/src/lib.rs#L552
[template-engine-api]: ../src/crates/concat-api/src/lib.rs#L705
[template-format]: ../src/crates/concat-host/src/templates.rs#L4
[template-dir]: ../src/crates/concat-host/src/templates.rs#L81
[template-list]: ../src/crates/concat-host/src/templates.rs#L170
[template-slots]: ../src/crates/concat-host/src/templates.rs#L225
[template-instantiate]: ../src/crates/concat-host/src/templates.rs#L274
[fill-slot]: ../src/crates/concat-project/src/commands/media.rs#L86
[app-dirs]: ../src/crates/concat-host/src/dirs.rs#L44
[apple-dirs]: ../src/crates/concat-host/src/dirs.rs#L252
[effects-packages]: ../src/crates/concat-effects/packages/
[text-presets]: ../src/crates/concat/text-presets/
[builtin-text]: ../src/crates/concat/src/presets.rs#L186
[shorts-styles]: https://github.com/AgriciDaniel/claude-shorts/blob/a369fad9d84287edbb8fa3ab515f268e770f683b/references/caption-styles.md
[shorts-components]: https://github.com/AgriciDaniel/claude-shorts/tree/a369fad9d84287edbb8fa3ab515f268e770f683b/remotion/src/components
[shorts-license]: https://github.com/AgriciDaniel/claude-shorts/blob/a369fad9d84287edbb8fa3ab515f268e770f683b/LICENSE

## 建议路线

**先交付本地可用的剪辑工作流和两套原创模板；素材站不上线也不阻塞。** 保留按上一章排序的 5 个原创 skill，但给长视频、口播和卡点先做手动时间码／节拍入口，不把自动分析伪装成已实现能力。[前一轮推荐](#推荐先做按顺序) · [现有 MCP 能力](MCP.md#what-the-agent-can-do)

| 阶段 | 任务与涉及范围 | 依赖及风险 |
| --- | --- | --- |
| 1. 原创 skill，先用本地素材 | 做「短片验收」与「多素材转竖版短片」，给「长素材转短片」「口播精剪与字幕」「音乐卡点混剪」定义手动输入和降级分支。新增 `src/crates/concat/agent-skills/`、调整 `concat/build.rs` 与 `concat/src/agent.rs` 的发现/安装入口；编写文本和自有样片，不复制上游文案。 | 依赖现有 MCP、本机 Codex 和用户同意安装范围；默认不覆盖同名技能。用两个项目核对发现和启用；逐帧预览只证明静态合成，不替代音频/播片质检。 |
| 2. 两套竖屏模板与声音策展（可与阶段 1 并行） | 先做「口播＋标题/字幕安全区」和「多镜头 B-roll／商品展示」原生模板，产物为原生 `template.json`、可选海报、已授权的 `assets/`；在 `concat-host/src/templates.rs` 增加内置来源及新旧置换规则，涉及 `concat/build.rs`、模板 UI 与必要的 `concat-api` / MCP 模板说明。声音单独在 `ConcatApp/assets/audio/` 策展 CC0 小包、审清单与摘要后发布镜像，沿用 `sounds.rs`；没有可再分发的配乐时留空槽。 | 依赖模板数量及资产分发许可确认、声音仓维护者授权发布；不覆盖用户模板，不把下载素材默认当作可打包资产。验证短素材、缺槽、错种类、分辨率、遮挡、字体缺失和迁移；避免工程指向缓存。 |
| 3. 分析能力补齐 | 给现有转写引擎加 `media_transcribe` / `media_transcript_get`，再加 `audio_detect_silence`；上线有真实词级时间码和置信信息的「长素材转短片」及「口播精剪与字幕」。随后补 `audio_analyze_beats` / `audio_analyze_sections` 和「音乐卡点混剪」。范围：`concat-speech`、相应宿主作业状态、`concat-api`、`concat-mcp`、`docs/MCP.md` 与原创 skill。 | 依赖本地模型可用、时间戳合同、项目音频提取和中断/失败处理；低置信度回退人工切点，保护笑声和语气；分析先只读，落剪用可撤销批次，音频分析权属与模型下载需明示。 |
| 4. 单一外站接入（可选） | 推荐先单独验证 Coverr：做 `stock_media_search` → 展示许可/来源 → 用户确认 → `media_download` → `media_import`，含下载统计和工程来源记录。范围：`concat-host` 媒体服务/凭证、`concat-api`／`concat-mcp`、资源 UI、`docs/MCP.md`；以后仅在许可和稳定性得到确认时扩站。 | 依赖用户选择供应商／档位、获授权 key、商用及署名规则核准。只发送必要查询，凭证不进日志，额度不足或政策变化回退本地文件；不要让 Agent 批量搜索下载、自动扣费或走生成／训练途径。Pexels 新发钥情况和 Pixabay 完整条款未解决前不把它们放到首发关键路径。 |

**需要用户拍板（进入开发前，不影响本调研结项）：**

1. 首发仅做本地，还是授权阶段 4 并行做 Coverr 验证？推荐首发不依赖外站；若接 Coverr，先确认使用档位、作者署名方式、谁持凭证和负责条款确认，以及只取 stock 还是允许 AI 生成库内容（默认只取 stock）。
2. 首批是否定为两套竖屏模板（口播、多素材展示）？推荐两套，先覆盖带字幕口播和镜头组接，再决定是否加横屏或直播模板。
3. 内置 skill 是否允许用户明确选择全局安装？建议默认按项目可用、另给一键全局选项；如要随 App 直接安装全局文件，必须先明示影响范围、覆盖策略和卸载方式。声音库则先只发布经核验、维护者批准的 CC0 条目。

**未覆盖：** 只做文档、公开状态和源码核对；没有试用 API 凭证、运行应用、渲染模板、下载样本或验证第三方集成商业协议。供应商显示的商业条款冲突或未核实项目，须在接入前复核。
