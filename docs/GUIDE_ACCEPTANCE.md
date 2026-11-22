# MindCut 操作手册独立验收

结论：12 项覆盖，11 项实际操作及预览成功；模板库为空，按要求跳过。发现 1 项“手册写错”：人声分离设置可通过通用 MCP 编辑命令接入，手册却笼统写成不可用。未发现本次已验证路径上的工具行为不符或工具缺陷。实际音频分离效果未验证，不能由设置写入成功推断。

## 环境与方法

- 任务：`guide-acceptance-run rev2`，计划 revision 33；执行日期：2026-11-19 America/Los_Angeles（日志日期 2026-11-20 UTC）。
- 基线 commit：`2ce79f69488f85f0ee796edda9466eeed72e241c`。
- 模式：**headless**，使用当前源码构建的 `/Users/elon/Agent/Concat/src/target/quick/concat --mcp headless`；不是 window 验收。
- 已完整阅读仓库内 `.claude/skills/run-concat/SKILL.md`（位置由主 Agent 澄清），从 `/Users/elon/Agent/Concat/src` 运行 `cargo build --profile quick -p concat`，exit 0，`Finished `quick` profile [optimized] target(s) in 4m 59s`。仅有 Slint 弃用属性/未使用组件警告。
- MCP 初始化返回 Concat `0.2.11`、协议 `2024-11-05`；GPU 为 `Apple M1 Pro (IntegratedGpu, Metal)`。
- 操作知识只来自 MCP `guide_read` 返回的 8 篇正文及 MCP 自身的工具说明/参数 schema；未读取业务实现或磁盘上的手册正文来补操作步骤。schema 另用于确认“未接入”的说法，不将通用命令的存在误当成渲染效果已验证。
- 依次读取目录及 `index`、`titles-captions`、`stickers-shapes`、`effects-filters-transitions`、`keyframes-masks`、`templates-export`、`workflow-verify`、`workflow-vertical-montage`，均成功。
- 前一角色已提交的单测未重复；本任务运行的是实际 MCP 编辑、合成预览和短片导出。

路径参数约定（下文参数中的 P、F 必须替换成这里的完整字符串，不是传给工具的变量名）：

~~~text
P = /Users/elon/eloncut-guide-verify-9un1mhrs/acceptance
F = /Users/elon/eloncut-guide-verify-9un1mhrs
~~~

子进程的 `HOME`、`TMPDIR`、`XDG_CONFIG_HOME`、`XDG_CACHE_HOME`、`XDG_DATA_HOME` 及工作目录都设为 F；服务启动日志确认：

~~~text
concat mcp: serving through {"mode":"headless","writesUnder":["/Users/elon/eloncut-guide-verify-9un1mhrs"]}
~~~

## 工程与素材

实际调用：

~~~text
project_create({"location":F,"name":"acceptance","video":{"width":720,"height":720,"rateNum":30,"rateDen":1}})
timeline_read({"path":P})
media_import({"path":P,"file":"/Users/elon/Agent/Concat/src/crates/concat-host/assets/card-still.jpg"})
media_import({"path":P,"file":"/Users/elon/Agent/Concat/src/crates/concat/ui/assets/motion-still.jpg"})
clip_add({"path":P,"mediaId":"m1","trackId":"T1","start":0})
clip_add({"path":P,"mediaId":"m2","trackId":"T1","start":5})
preview_frame({"path":P,"time":1,"output":"/Users/elon/eloncut-guide-verify-9un1mhrs/00-base.png"})
~~~

返回：`m1` 为 720×720 JPG，`m2` 为 192×192 JPG；均为仓库现成的同一人物静帧、分辨率不同，未联网下载。两段图片片段 `c3=[0,5)`、`c4=[5,10)`，默认静图时长均为 5 s；工程 `720×720 @ 30/1`，总长 10 s。创建后即先读时间线，使用返回的 `T1`、`m1`、`m2`、`c3`、`c4` 等真实 ID。

字幕是本验收明确指定的测试字符串及时间码，不是对素材语音的转写。

## 逐项结果

每次预览实际使用 `preview_frame({"path":P,"time":时间,"output":完整PNG路径})`，未覆盖已有用户文件。以下“无”表示该项实际调用未报错；额外的负向探测错误另列。图片在清理前经图像查看工具检查，文件随后按任务要求删除。

| 项目 | 手册是否讲清 | 实际工具与参数（除 P 外均为实际值） | 成败与核对结果 | 报错原文 | 验证帧路径/时间 |
| --- | --- | --- | --- | --- | --- |
| 加文字 | 是：开始、时长、叠放、字体大小/颜色均明确 | `title_add({"path":P,"start":0,"duration":4,"above":true,"offsetY":-0.3,"style":{"content":"Guide Acceptance","fontSize":0.08,"fontWeight":700,"color":"#ffffff"}})` | 成功，`createdId=c5`；白色标题出现在画面上部 | 无 | `/Users/elon/eloncut-guide-verify-9un1mhrs/01-title.png`，1 s |
| 按时间码加字幕 | 是：`start/end/text`、位置与默认样式明确 | `captions_add({"path":P,"captions":[{"start":0.5,"end":1.5,"text":"First test caption"},{"start":1.5,"end":2.5,"text":"Second test caption"}],"offsetY":0.35})` | 成功，`c6=[0.5,1.5)`、`c7=[1.5,2.5)`；1 s 显示第一句，1.6 s 显示第二句 | 无 | `/Users/elon/eloncut-guide-verify-9un1mhrs/02-caption-a.png`，1 s；`/Users/elon/eloncut-guide-verify-9un1mhrs/02-caption-b.png`，1.6 s |
| 部分文字换样式 | 是：Unicode 标量索引、右端不含、patch 明确 | `title_style_words({"path":P,"clipId":"c5","start":0,"end":5,"patch":{"color":"#ffd400","fontWeight":900}})` | 成功，`textRuns=[{"color":"#ffd400","end":5,"fontWeight":900,"start":0}]`；仅 Guide 变黄，Acceptance 仍白色 | 无 | `/Users/elon/eloncut-guide-verify-9un1mhrs/03-word-style.png`，1 s |
| 加贴纸/形状 | 是：示例可直接执行 | `edit_apply({"path":P,"command":{"op":"addShapeClip","above":true,"start":2,"duration":3,"name":"Yellow callout","style":{"kind":"square","width":0.35,"height":0.14,"cornerRadius":0.3,"fill":"#ffd400"}}})` | 成功，`c8` 在 `T4`、[2,5)；黄色圆角矩形可见。示例默认居中会遮挡人物，属于明确位置设置的结果 | 无 | `/Users/elon/eloncut-guide-verify-9un1mhrs/04-shape.png`，3 s |
| 加转场 | 是：目录 ID 与 incoming clip 写法明确；需自行采样判断区间 | 先 `effects_list({"kind":"transition"})`；`clip_update({"path":P,"clipId":"c4","patch":{"transitionIn":{"id":"concat.dissolve","duration":1}}})`；采样后改为 `clip_update({"path":P,"clipId":"c4","patch":{"transitionIn":{"id":"concat.fade-black","duration":1}}})` | 两次成功，最终读回 fade-black、1 s；5 s 黑场可见。两张素材内容相同，仅分辨率不同，因此用黑场确认转场；未验证连续播放 | 无 | `/Users/elon/eloncut-guide-verify-9un1mhrs/05-transition-0.png` 至 `/Users/elon/eloncut-guide-verify-9un1mhrs/05-transition-4.png`，依次 4.8/5/5.25/5.75/6.2 s（dissolve）；`/Users/elon/eloncut-guide-verify-9un1mhrs/05-transition-black.png`，5 s |
| 加效果 | 是：`videoEffects` 整链替换明确 | `effects_list({"kind":"effect"})` 得到 `concat.gaussian-blur`、`radius=1..50 px`；`clip_update({"path":P,"clipId":"c3","patch":{"videoEffects":[{"id":"concat.gaussian-blur","params":{"radius":20}}]}})` | 成功，人物底图明显模糊，上层标题/字幕仍清晰 | 无 | `/Users/elon/eloncut-guide-verify-9un1mhrs/06-effect.png`，1 s |
| 加滤镜 | 是：视频滤镜与音频 filters 区分清楚 | `effects_list({"kind":"filter"})` 得到 `concat.warm`、`temperature=3000..6000 K`；`clip_update({"path":P,"clipId":"c3","patch":{"videoEffects":[{"id":"concat.gaussian-blur","params":{"radius":20}},{"id":"concat.warm","params":{"temperature":3000}}]}})` | 成功，保留模糊，底图变暖；读回两项有序链 | 无 | `/Users/elon/eloncut-guide-verify-9un1mhrs/07-filter.png`，1 s |
| 调节图层并调参数 | 是：先查 concat.adjust，再建图层并改其链 | 同一 filter 目录返回 `concat.adjust`、`exposure=-3..3 EV`；`effect_layer_add({"path":P,"effectId":"concat.adjust","name":"Acceptance adjust","start":0,"duration":2})` → `c9`；`clip_update({"path":P,"clipId":"c9","patch":{"videoEffects":[{"id":"concat.adjust","params":{"exposure":1}}]}})` | 成功，`c9` 位于 `T4`、[0,2)，高于受影响画面；读回 exposure=1，画面明显增亮 | 无 | `/Users/elon/eloncut-guide-verify-9un1mhrs/08-adjust.png`，1 s |
| 片段关键帧 | 是：归一化 at 与属性单位明确 | `keyframe_set({"path":P,"clipId":"c4","property":"scale","at":0,"value":1})`；`keyframe_set({"path":P,"clipId":"c4","property":"scale","at":1,"value":1.5})` | 成功，读回两端 scale 关键帧；6.2/7.5/9.8 s 采样看到逐步放大；未声称验证运动流畅度 | 无 | `/Users/elon/eloncut-guide-verify-9un1mhrs/09-keyframe-0.png`、`/Users/elon/eloncut-guide-verify-9un1mhrs/09-keyframe-1.png`、`/Users/elon/eloncut-guide-verify-9un1mhrs/09-keyframe-2.png`，依次 6.2/7.5/9.8 s |
| 加蒙版 | 是：替换数组、ID、source 形状及尺寸明确 | `clip_masks({"path":P,"clipId":"c4","masks":[{"id":"acceptance-mask","source":{"kind":"shape","shape":"circle","width":0.66,"height":0.66,"roundness":0}}]})` | 成功，`project_get` 读回该圆形蒙版；画面圆外黑色、圆内保留人物，随片段缩放 | 无 | `/Users/elon/eloncut-guide-verify-9un1mhrs/10-mask.png`，7.5 s |
| 套模板 | 是：列目录→填本地槽位→新工程，说明清楚 | `template_list({})` | 返回 `[]`；此隔离用户目录没有可用/内置模板，按任务要求跳过 `template_instantiate`，未虚构模板或槽位 | 无 | 无；库为空，没有可套模板的帧 |
| 导出一小段 | 是：job 不等于完成、wait/status 与超时语义明确 | `export_start({"path":P,"output":"/Users/elon/eloncut-guide-verify-9un1mhrs/acceptance.mp4"})` → `j1`；`export_wait({"job":"j1","timeoutSeconds":30})` | 成功，`state=done,fraction=1,frame=300,total=300,width=720,height=720`；10 s 文件存在且可解析 | 无 | `/Users/elon/eloncut-guide-verify-9un1mhrs/11-export-final.png`，7.5 s；这是最终时间线 MCP 合成帧，不冒充导出视频抽帧 |

`project_get({"path":P})`（MCP id 54）同时确认：文字及 runs、两条字幕时段、形状参数、两项视频效果链、调节图层 exposure=1、c4 两个 scale 关键帧及 circle 蒙版；`timeline_read`/编辑返回的摘要确认分层与总时长。没有依靠“工具成功”代替画面检查。

## 手册“做不到”逐项核对

核对依据：初始化返回的完整工具列表、`tools/list` 的输入 schema，包含 `edit_apply` 下全部 **58** 个 `Command.op`，以及下列最小动态探测。没有读取实现代码推测能力；“未接入”仅限本次 MCP 公共表面，不是断言桌面 UI 没有该功能。

| 手册限制 | 核对证据 | 结论 |
| --- | --- | --- |
| 自动转写/自动字幕、语音生成 | 工具表及 58 个 op 均无转写/合成请求；`captions_add` 必须有非空 `{start,end,text}` 数组 | 未接入；本次字幕为指定测试输入 |
| 人声分离/voice isolation | `edit_apply` 暴露 `setClipVoice`；`VoiceMix.kind` 支持 `speech`、`vocals`。含音轨 fixture 实际调用后读回 voice 配置，见问题 G-01 | **与手册不一致：设置入口已接入；实际分离渲染/听感未验证** |
| 画质增强工作流 | 没有生成增强副本的工具/op；但存在 `setClipEnhanced`，说明为切换已有增强副本。无副本的 fixture 调用 on=true 不报错，读回未新增增强字段 | 生成增强副本未接入；“没有任何增强相关入口”会过宽。现有 toggle 不证明能从 MCP 完成增强 |
| 声音库、素材/stock 库搜索与整理 | 无库搜索/获取/分组工具或 op；`media_import` 接本地文件；`removeMedia`、`updateMediaPath` 只操作现有工程素材箱 | 专用库能力未接入；工程素材导入/删除/重链接入，不能混为一谈 |
| 预设卡片按展示名调用 | 文字/形状命令是显式样式，无 preset-card 名参数或对应 op；效果工具需目录 id | 未接入；本次名称只是片段标签 |
| 贴纸搜索、动画贴纸库 | 无 sticker-search/库接口；仅 `addShapeClip` 和导入已有本地图片的通路 | 未接入 |
| 未列入目录的效果 | 只有目录包 id 的效果链/图层路径，无安装或查找未知效果的工具 | 未接入；不把静默跳过未知 id 当成支持 |
| 自动主体追踪 | 58 个 op 无 tracker/自动跟踪请求；mask 与 key 工具只写显式形状和人工关键帧；自动 cutout 设置不等于跟踪请求 | 未接入自动追踪请求 |
| 任意属性动画 | `keyframe_set.property` 是 scale/offsetX/offsetY/rotation/opacity/volume 枚举；效果参数键与蒙版 number 有各自目标 | 未接入任意属性动画；已验证支持列表中的 scale |
| 保存模板 | 无 `template_save` 工具/op；实际探测 `template_save({})` | 确认未接入，原始 JSON-RPC 错误见下 |
| 模板槽位取 URL 或 stock/声音/素材库 | `template_instantiate.fills` 只提供 `{mediaId,file}`，工具说明明确为本地文件且先 probe；无下载或 stock resolver 请求 | 本地文件通路接入；未运行网络下载，也没有模板可实测 URL 拒绝 |
| 自动高光挑选、节拍对齐、生成素材 | 工具表与全部 op 无高光分析/节拍分析/生成画面请求 | 未接入；未把手动剪切/排列或音频效果当作自动分析 |
| 静帧不能证明声音、连续运动、节奏、口型同步 | `preview_frame` 只返回一个时刻的合成图像 | 限制成立；本报告只声明采样帧、结构与文件容器验证 |

保存模板负向探测（MCP id 66）：

~~~json
{"error":{"code":-32602,"message":"Unknown tool: template_save"},"id":66,"jsonrpc":"2.0"}
~~~

## 问题分类与复现

### 手册写错

**G-01：将人声分离笼统写为 MCP 不提供，遗漏通用 edit_apply 的设置入口。**

影响篇目：`index`（“do not provide ... vocal isolation”）、`keyframes-masks`（“nor isolate a voice”）、`workflow-verify`（“vocal isolation ... unavailable”）；其他篇目的“see the index”也会传播该限制。

复现步骤：

1. `guide_read({"topic":"index"})`，核对上述限制；再查 MCP 工具 schema 中 `edit_apply.Command.setClipVoice` 与 `VoiceMix`。其描述为 “Sets a clip's voice isolation - the voice and the rest at levels of their own - or takes it off.”
2. 在本验收工程临时导入仓库已有的含音轨 fixture：`media_import({"path":P,"file":"/Users/elon/Agent/Concat/src/crates/concat-media/fixtures/sound-starts-late.mov"})` → `m10`；`clip_add({"path":P,"mediaId":"m10","trackId":"T1","start":10})` → `c11`。`ffprobe` 与 import 均确认该视频长 1.500000 s、32×32、H.264，含 pcm_u8 音轨。
3. 实际调用（MCP id 59）：

~~~text
edit_apply({"path":P,"command":{"op":"setClipVoice","clipId":"c11","voice":{"kind":"speech","voice":1,"background":0}}})
project_get({"path":P})
~~~

4. 调用 `isError=false`；`project_get`（MCP id 60）读回 `c11.voice={"background":0,"kind":"speech","voice":1}`，证明公共 MCP 命令接收并应用该配置。该字段没有出现在仅对静图 c4 的早期探测中，因此改用含音轨的视频复核，未把静图上的成功返回当证据。
5. 此检查未启动语音模型、未渲染/试听分离音轨，未联网下载模型。它证明“设置入口接入”，**不证明完整声音分离在当前环境可用或有效**。
6. 用 `edit_apply({"path":P,"command":{"op":"batch","commands":[{"op":"removeClips","clipIds":["c11"]},{"op":"removeMedia","mediaId":"m10"}]}})` 清除探测片段与素材；摘要确认回到原来的 2 素材、7 片段、10 s，再导出。

建议：手册明确区分“没有专用 MCP 工具”“可经 edit_apply 设置 speech/vocals 混音”“实际模型/缓存与渲染能力需另行确认”，不要将三者一并称为未接入。错误原文：无（此问题以成功接入与文案冲突为证据）。预览帧：不适用，声音功能不能用静帧证明。

### 工具行为与手册不符

无独立问题。G-01 归为手册写错，不重复计数。图像编辑相关实际行为与对应说明一致。

### 工具缺陷

无。已执行路径没有返回业务报错、崩溃或预览失败；这不表示未覆盖的操作没有缺陷。

附加边界探测：`edit_apply({"path":P,"command":{"op":"setClipEnhanced","clipId":"c11","on":true}})` 成功返回，但在无增强副本的素材上读回无增强字段，不能作为画质增强已完成的证据。未修改实现或手册。

## 导出证据

~~~text
export_start -> {"job":"j1","output":"/Users/elon/eloncut-guide-verify-9un1mhrs/acceptance.mp4","path":"/Users/elon/eloncut-guide-verify-9un1mhrs/acceptance"}
export_wait -> {"state":"done","fraction":1,"frame":300,"total":300,"width":720,"height":720}
~~~

运行：

~~~sh
ffprobe -v error -show_entries format=duration,size:stream=codec_name,width,height,r_frame_rate,nb_frames -of json '/Users/elon/eloncut-guide-verify-9un1mhrs/acceptance.mp4'
~~~

实际输出要点：

~~~json
{"streams":[{"codec_name":"h264","width":720,"height":720,"r_frame_rate":"30/1","nb_frames":"300"}],"format":{"duration":"10.000000","size":"1898509"}}
~~~

MP4 SHA-256：`a93a62fd6dcc74b6d07532317a7c4dacbb952cc246363d362bb79596753bd96a`。日志确认 `h264_videotoolbox (hardware)`、300 帧、`done in 819 ms`。未播放 MP4、未抽帧对比导出像素或检查听感；最终预览帧仍来自 MCP 时间线。

## 预览帧清单

以下是清理前实际生成的 PNG 文件及 SHA-256；**当前均已删除**，路径用于定位本次验收记录，不是可继续打开的交付物。已人工查看 base、title、两条 caption、word-style、shape、transition 的 0/2/4 及 black、effect、filter、adjust、三个 keyframe、mask。其余转场采样也生成成功；export-final 与已查看的 mask 哈希完全一致。

| 实际路径 | bytes | SHA-256 |
| --- | ---: | --- |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/00-base.png` | 829611 | `57b0027b52b7c0a53a11c99b90351f78273f50bdbcd3aa5f945eee847cd209d9` |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/01-title.png` | 837748 | `fc0f60ce37df57e31bbf78113774cb4229a08e3ad3914a12152a074080812c80` |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/02-caption-a.png` | 838607 | `a7b45747e4b6c046f7b11a6847ada660eb9aff572b513e9f5d942770bedb662a` |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/02-caption-b.png` | 839217 | `826932713c045e81646389153fda56da07da99a25d22600b7241a38097449459` |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/03-word-style.png` | 838016 | `74d2ae1a19a8a1ef2a646aab9088bf55d65b5a65a8e5580b354b8793b2f6a63f` |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/04-shape.png` | 778782 | `5f34d00436429a2dae563271c828f70a1bc0071d85fd5479447c806d40a77e0b` |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/05-transition-0.png` | 778009 | `54cde0860fa36dd474b9e419c90af0ccc8a6652e0cea52c706f8200b51ef2cb5` |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/05-transition-1.png` | 816403 | `ba0381bdc0aba5c47ae21acd2111e05b72744094afbe34f36300987aa42d1e9b` |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/05-transition-2.png` | 781827 | `5cf738409b061fe977c06489754c334b62890e24ba8800ed88e49688dbd663c7` |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/05-transition-3.png` | 747329 | `56b0a6f93b33e4becc5bcaa72640047f39d0fecce331324bec1c4c680f4fd631` |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/05-transition-4.png` | 747329 | `56b0a6f93b33e4becc5bcaa72640047f39d0fecce331324bec1c4c680f4fd631` |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/05-transition-black.png` | 12807 | `8b4699d0171e1f56f2d5a29dca16e886313a9a3dbeed5bb3a3fe9596bc7842f7` |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/06-effect.png` | 514265 | `1a5483dbd077a0024ab1d29d6665b9318d12378412c766a7daebbd3832279890` |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/07-filter.png` | 459590 | `6b44d0317001687409e3166216a7f6864612a3bddc2ba5567e012fb1e0ba7fba` |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/08-adjust.png` | 437291 | `fdb8dcc87f326a27a6f416aab648965acfb07b3db33766010af0c8e109f102d8` |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/09-keyframe-0.png` | 773060 | `12d08cfd7728da72b3908db54dcb4966ea9789ba8a753ddec35764dfa2b14fea` |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/09-keyframe-1.png` | 790875 | `0b66dde1dae17585fa83cf0d45fc94465786acc975607507e830d5bd99eb2443` |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/09-keyframe-2.png` | 794621 | `af54302b846cff0e933c6f97f55ccbff5039f340838f9a5675314b5facd3608a` |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/10-mask.png` | 488164 | `584415730f7346ff905bf871dce780138f19850cdb1143a24779b375afa4cd5f` |
| `/Users/elon/eloncut-guide-verify-9un1mhrs/11-export-final.png` | 488164 | `584415730f7346ff905bf871dce780138f19850cdb1143a24779b375afa4cd5f` |

## 清理与未覆盖风险

- `project_close({"path":P})` 返回 `isError=false`。
- 自己启动的 headless 子进程 PID `35111` 关闭 stdin 后正常退出，`exit=0`；Python MCP 桥接进程已结束，Cargo 构建进程已 exit 0。没有留下本 session 的应用或导出进程。
- 删除整个 `/Users/elon/eloncut-guide-verify-9un1mhrs`（工程、配置/缓存、标题缓存 PNG、20 张验证帧、MP4）；清理检查返回 `exists=false`。没有删除仓库自带素材。
- 启动前曾探测 `concat --help`，应用不支持该 flag，exit 1，提示 usage；它意外在默认位置写出一份日志 `/Users/elon/Library/Application Support/app.concat.editor/logs/concat-20261120-041601.log`。此自建探测日志已单独删除并确认 `exists=false`；随后正式验收全部隔离在 F。没有把这个前置探测当成功启动。
- 仓库交付只新建本文件；未改业务代码、手册、其他终端修改或暂存。Cargo 更新的是正常构建缓存，plan 更新的是受管进度记录。
- 未验证 window/UI 页签交互、连续播放、声音/语音分离效果、mask/effect 参数动画、音频效果层、独立转场片段、所有目录效果、模板实例化或保存（无内置模板），以及导出视频像素与听感。采样静帧不支持这些结论。
- 因清理要求未保留可重开的临时工程、PNG 或 MP4；本文件保留可复现参数、返回关键字段、原始错误、完整生成路径与内容哈希。
