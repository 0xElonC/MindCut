# AI 工具独立验收

结论：按任务 rev2 的缺模型验收口径完成。7 个新工具均已通过实际 MCP 调用覆盖；参数错误、四类模型缺失及无已存转写的错误符合手册，未发现已测路径上的问题。本机四类模型均未安装，4 个 AI 启动接口都在创建 job 前返回 modelMissing。**真实任务的进度轮询与取消未覆盖**，这部分只沿用开发任务已交付的夹具证据。真实推理全部“未做真实运行”，不将不存在 job 的错误处理或夹具证据冒称本次真实生命周期通过。

## 环境、范围与构建

- 任务：`ai-tools-acceptance rev2`，#33，计划 revision 47；2026-11-22 America/Los_Angeles。已重新 plan start 采用 rev2；主 Agent 选 A，明确允许缺模型时引用已交夹具证据并将真实生命周期记为未覆盖，沿用已完成构建及动态检查，不下载模型、不重跑构建。
- 模式：**headless MCP**，`concat --mcp headless`；未运行 window。
- 基线 commit：`c4573a8e72a2b4378d58a96a544ee8649e25d572`。
- 仓库开始时有其他会话的未提交改动，包括 `concat-api/src/lib.rs`、`concat-api/src/speech.rs`、`concat-speech/src/tts.rs` 及 Agent 接入文件。本次基于当前工作树构建并验收，不覆盖这些改动；关键文件在构建/验收期间的 SHA-256 一致，见末表。
- 完整读取 `.claude/skills/run-concat/SKILL.md`；在 `/Users/elon/Agent/Concat/src` 运行 `cargo build --profile quick -p concat`，exit 0，输出 ``Finished `quick` profile [optimized] target(s) in 5m 04s``；仅有既有 Slint 警告。
- 可执行文件：`/Users/elon/Agent/Concat/src/target/quick/concat`；初始化返回 Concat `0.2.11`、协议 `2024-11-05`，7 个新工具全部出现在 tools/list。
- 操作流程只依据实际 `guide_read` 返回的目录、`ai-tools`、`index`、`workflow-verify`；未从磁盘手册或业务实现补操作步骤。MCP 自身参数 schema 用于类型/参数测试；源码只用于任务明确要求的“不下载模型”确认及运行目录隔离。
- 没有重跑开发角色已经交证据的单测；没有新增测试或修改业务代码/手册。

目录：

~~~text
F = /Users/elon/eloncut-ai-verify-zu0mccyt
P = /Users/elon/eloncut-ai-verify-zu0mccyt/acceptance
~~~

下文参数里的 P/F 是完整路径的书写约定，实际请求传的是对应完整字符串。

先启动正常目录的 headless 进程，只调用只读工具，取得**真实用户目录**的 ai_status，随后正常停止，未创建真实用户工程。再将同一构建的可执行文件用本地 APFS clone `cp -c` 放在 F/bin/concat，创建其旁的 F/bin/portable，使用应用原有 portable 目录机制隔离配置、最近工程及缓存。没有改变 HOME。正式错误路径验收使用此 portable headless 进程，`CONCAT_MCP_ROOTS=F`、`TMPDIR=F`。两次启动日志均为 `no file this run`，写入范围均为 F。

## 四类模型状态

真实目录的 `ai_status({})`（MCP id 4）和 portable 目录的前后两次 `ai_status({})`（id 11、42）除 modelsDir 外一致：

| 类别 | status | installed | busy | 其他活动字段 | 真实 modelsDir |
| --- | --- | --- | --- | --- | --- |
| transcription | available | false | false | transcribing=false，downloading=false | /Users/elon/Library/Application Support/app.concat.editor/whisper-models |
| speechGeneration | available | false | false | synthesizing=false，downloading=false，systemVoiceAvailable=false | /Users/elon/Library/Application Support/app.concat.editor/tts-models |
| vocalSeparation | available | false | false | 两个模型 installed=false | /Users/elon/Library/Application Support/app.concat.editor/cutout-models |
| videoEnhancement | available | false | false | 模型 installed=false | /Users/elon/Library/Application Support/app.concat.editor/cutout-models |

实际返回的模型列表（字节数原值）：

| 类别 | 模型 id | sizeBytes | 下载/安装状态 |
| --- | --- | ---: | --- |
| transcription | tiny.en | 77704715 | downloaded=false，englishOnly=true |
| transcription | tiny | 77691713 | downloaded=false，englishOnly=false |
| transcription | base.en | 147964211 | downloaded=false，englishOnly=true |
| transcription | base | 147951465 | downloaded=false，englishOnly=false |
| transcription | small.en | 487614201 | downloaded=false，englishOnly=true |
| transcription | small | 487601967 | downloaded=false，englishOnly=false |
| speechGeneration | chatterbox-turbo-q8 | 1123738092 | downloaded=false |
| vocalSeparation | spleeter-2stems-vocals | 19681017 | installed=false |
| vocalSeparation | spleeter-2stems-accompaniment | 19681024 | installed=false |
| videoEnhancement | realesr-general-x4v3 | 4866417 | installed=false |

不是因 portable 隔离而把本机已安装模型误报为缺失：正常目录 ai_status 已确认全部缺失；只读文件检查也确认真实 `whisper-models`、`tts-models` 存在但无文件，`cutout-models` 不存在。检查在验收前后结果相同。

`speech_voices({})`（id 17）返回 5 条均 available=false、isClone=false：

| id | name | family | modelId | requiresModel |
| ---: | --- | --- | --- | --- |
| 2000 | af_elizabeth | chatterbox | chatterbox-turbo-q8 | true |
| 2001 | am_mark | chatterbox | chatterbox-turbo-q8 | true |
| 2002 | bf_ruth | chatterbox | chatterbox-turbo-q8 | true |
| 2003 | xm_bob | chatterbox | chatterbox-turbo-q8 | true |
| 3000 | system_default | system | system-voice | false |

2000–2003 的 modelInstalled=false；3000 没有可用离线 provider，因此没有借系统声线绕过模型缺失做合成。

## 本地测试工程

实际调用：

~~~text
project_create({"location":F,"name":"acceptance","video":{"width":320,"height":180,"rateNum":30,"rateDen":1}})
timeline_read({"path":P})
media_import({"path":P,"file":"/Users/elon/Agent/Concat/src/crates/concat-media/fixtures/sound-starts-late.mov"})
media_import({"path":P,"file":"/Users/elon/Agent/Concat/src/crates/concat/ui/assets/motion-still.jpg"})
clip_add({"path":P,"mediaId":"m1","trackId":"T1","start":0})
clip_add({"path":P,"mediaId":"m2","trackId":"T1","start":1.5})
~~~

返回：m1 是带音轨的 32×32 视频、1.5 s，c3=[0,1.5)；m2 是 192×192 静图，c4=[1.5,6.5)。使用的是目录/时间线返回的真实 ID。没有下载素材、模型或生成假模型标记，也没有伪造转写文件。

## 逐工具验收

“符合”指在**模型未安装场景**中符合预期，不表示真实推理通过。“参数错误”列给出实际额外参数/调用；模型错误原文统一列在后节，避免改写。

| 工具 | 手册是否讲清 | 实际调用与参数 | 成败/结果 | 报错原文或索引 | 真实运行/产物 |
| --- | --- | --- | --- | --- | --- |
| ai_status | 是：四个类别、available 与 installed 区分明确 | `ai_status({})`；额外负测通过 tools/call 传 `arguments:"invalid-arguments"` | 正常调用成功，四类均 installed=false；非对象 arguments 被拒绝。此工具无业务参数，不应要求它返回 modelMissing | id 18：`a tool's arguments are an object`，JSON-RPC -32602 | 状态真实读取；不执行推理、不生成文件 |
| media_transcribe | 是：二选一目标、语言/modelId、job 与源时间说明明确 | `media_transcribe({"path":P})`；`{"path":P,"mediaId":"m1","clipId":"c3"}`；`{"path":P,"clipId":"c3","language":"not-a-language"}`；`{"path":P,"clipId":"c3","modelId":"not-a-model"}`；有效形状 `{"path":P,"clipId":"c3","modelId":"tiny.en","language":"en"}` | 参数错误和 modelMissing 均符合；有效形状调用在启动前拒绝，没有 job | id 20/21：`supply exactly one of mediaId and clipId` (invalid)；id 22：`unknown Whisper language code` (invalid)；id 23：`unknown Whisper modelId; see ai_status` (invalid)；id 24：M1 | **未做真实运行**；无 transcript/词级时间码产物 |
| transcript_query | 是：查询已存转写，源时间 start/end、keyword 语义明确 | `transcript_query({"path":P})`；`{"path":P,"clipId":"c3","start":2,"end":1}`；`{"path":P,"clipId":"c3","keyword":"test"}` | 缺目标返回 invalid；已有目标但无转写返回 notFound。无文件时先返回缺转写，未验证 start>end 的后续处理。查询本身不加载模型，无 modelMissing 要求 | id 25：`supply exactly one of mediaId and clipId` (invalid)；id 26/27：`no saved transcript for this target; run media_transcribe first` (notFound) | **未做真实转写结果查询**；不伪造 transcript 补结果 |
| speech_voices | 是：用 available 条目，voiceId 来自列表 | `speech_voices({})`；额外负测传 `arguments:"invalid-arguments"` | 正常列 5 声线，全部 available=false，无克隆；非对象 arguments 拒绝。只读列表不应因缺模型整体报错 | id 19：`a tool's arguments are an object`，JSON-RPC -32602 | 列表真实读取；未做合成 |
| speech_generate | 是：text/voiceId/speed、自动导入及需自行入轨明确 | `speech_generate({"path":P,"text":"","voiceId":2000})`；`{"path":P,"text":"Acceptance test.","voiceId":2000,"speed":0}`；voiceId=-1；voiceId=3000；有效形状 `{"path":P,"text":"Acceptance test.","voiceId":2000,"speed":1}` | 空文本/越界速度/未知或无离线能力声线均拒绝；2000 返回 modelMissing，没有 job | id 28：`speech text is empty` (invalid)；id 29：`speed must be between 0.5 and 2` (invalid)；id 30：`unknown voiceId; choose a voice from speech_voices` (voiceUnavailable)；id 31：`no offline system voice is available on this device` (voiceUnavailable)；id 32：M2 | **未做真实运行**；没有 WAV、mediaId 或新增入轨 |
| audio_separate_vocals | 是：目标二选一、分离任务不改变混音、完成后 setClipVoice 明确 | `audio_separate_vocals({"path":P})`；`{"path":P,"mediaId":"m1","clipId":"c3"}`；有效形状 `{"path":P,"clipId":"c3"}` | 前两项 invalid；带音轨片段返回 modelMissing，没有 job | id 33/34：`supply exactly one of mediaId and clipId` (invalid)；id 35：M3 | **未做真实运行**；没有 voice.wav/rest.wav、完成状态或混音效果 |
| clip_enhance | 是：video/image 目标、scale 范围、缺模型入口、耗时/缓存说明明确 | `clip_enhance({"path":P,"clipId":"c4","scale":0.5})`；scale=5；有效形状 `{"path":P,"clipId":"c4","scale":2}` | 0.5 与 5 均 invalid；2 返回 modelMissing，没有 job | id 36/37：`scale must be between 1 and 4; omit it for automatic scale` (invalid)；id 38：M4 | **未做真实运行**；没有增强 PNG/MP4 或片段替换 |

每个业务错误都返回 `result.isError=true` 和 `structuredContent.error={code,message}`；非对象参数是外层 JSON-RPC -32602。没有将预期拒绝记为产品缺陷。

## 缺模型结构化错误原文

以下为实际 `structuredContent.error`，消息原样保留。包含缺少的模型 ID、大小与用户自己操作的安装入口；Spleeter/增强的入口在片段检查器，不能错误地改报为 Settings。

M1，media_transcribe，MCP id 24：

~~~json
{"code":"modelMissing","message":"Whisper model tiny.en is not installed (approximately 77704715 bytes). Download it in ElonCut Settings > Transcriber, then retry."}
~~~

M2，speech_generate，MCP id 32：

~~~json
{"code":"modelMissing","message":"Speech model chatterbox-turbo-q8 is not installed (approximately 1123738092 bytes). Download it in Settings > Speech, then retry."}
~~~

M3，audio_separate_vocals，MCP id 35：

~~~json
{"code":"modelMissing","message":"Missing Spleeter model(s): spleeter-2stems-vocals, spleeter-2stems-accompaniment. The pair (spleeter-2stems-vocals, spleeter-2stems-accompaniment) totals 39,362,041 bytes. In the window inspector, select Voice Isolation > Song (人声分离 → 歌曲) to download, then retry."}
~~~

M4，clip_enhance，MCP id 38：

~~~json
{"code":"modelMissing","message":"Missing realesr-general-x4v3 (4866417 bytes). In the window's clip inspector, turn on Enhance and click Preview to download it."}
~~~

## 不联网下载的确认依据

本次选择**读代码核对**，没有声称做过抓包或全进程网络监控。以下是运行到的缺模型路径：

| 路径 | 已核对的源码及行为 |
| --- | --- |
| 状态/声线 | `src/crates/concat-api/src/transcription.rs:267`、`speech.rs:100`、`speech.rs:122` 使用 status_read_only；`concat-speech/src/transcribe.rs:1130`、`tts.rs:519` 读取已有模型状态，无下载调用 |
| 转写 | `concat-api/src/transcription.rs:213` 的 choose_model 在 downloaded=false 时返回 ModelMissing；`:306` 在创建 TimedRun/后台 job 之前调用它。引擎后续 `concat-speech/src/transcribe.rs:1271` 的 run_in_slot 也在模型文件不存在时直接返回；下载入口 download_model 是另一方法 `:1157` |
| 配音 | `concat-api/src/speech.rs:52` 的 stock_model 在模型未下载时返回 ModelMissing；`:152` 的 generate_speech 在 begin_stock/job 之前检查。后续 StockRun::speak `concat-speech/src/tts.rs:454` 走指定 target 的本地合成路径；`:714` 用已有目录，`:719` 再查模型，不调用 download_model |
| 人声分离 | `concat-api/src/vocals.rs:30` 的 local_networks 只调 Networks::installed，未安装直接 ModelMissing；separate_vocals 在启动 worker 前执行该检查（`:183`）。`concat-speech/src/vocals.rs:85` 的 installed 只找本地两文件；成功路径 separate 接收现有 Networks |
| 画质增强 | `concat-api/src/enhance.rs:146` 在启动任务前检查 installed，`:192` 在线程里再次检查并调用 enhance_local；`concat-host/src/enhance.rs:300` 的 enhance_local 给 run 传 local_cancel，run/loaded（`:261` 起）由此设置 allow_download=false，缺失即返回；只有窗口另一入口可允许 fetch |

同时观察到：

- 4 个启动调用均立即返回 modelMissing，没有返回 job，后续 `ai_status` 仍 installed=false/busy=false；转写/配音的 downloading=false。
- portable 临时目录的完整文件扫描仅有克隆的 executable、portable/recents.json 和测试工程 manifest/lock，没有模型目录、`.part`、WAV、transcripts、分离或增强缓存产物。
- 真实模型目录前后均为空/不存在，未出现新增权重文件。

这些依据支持本次调用没有下载模型；不将“未见文件”单独当作“没有网络”的证明。

## 任务轮询与取消：夹具证据及真实覆盖限制

真实调用：

~~~text
export_status({"job":"verify-no-such-ai-job"})
export_cancel({"job":"verify-no-such-ai-job"})
export_status({"job":"verify-no-such-ai-job"})
~~~

实际错误：

~~~json
{"code":"notFound","message":"no job verify-no-such-ai-job: it never ran, or ended long ago"}
{"code":"notFound","message":"no job verify-no-such-ai-job is running"}
{"code":"notFound","message":"no job verify-no-such-ai-job: it never ran, or ended long ago"}
~~~

仅证明不存在任务时的查询/取消拒绝行为。没有真实 AI job，因此未验证 running → 进度变化 → done/cancelled，也未验证取消后的产物清理或任务隔离；不能拿普通视频导出任务替代 4 类 AI 任务。

**真实任务的进度轮询与取消未覆盖。** 按 revision 47 的验收第 3 条，轮询/取消只覆盖以下开发任务已交付的夹具证据；本次仅核对测试位置，没有重复执行这些测试：

- ai-transcribe-tools：`src/crates/concat-api/src/transcription.rs:548`，`transcription_job_reports_progress_and_cancel_does_not_stop_another_job`，开发汇报涵盖进度/轮询/定向取消。
- ai-speech-tools：`src/crates/concat-api/src/speech.rs:469`，`generated_audio_is_probed_imported_and_never_placed_on_the_timeline`，WAV 夹具验证 running/fraction=0.25 → done、mediaId 与 duration；`:546` 的 `cancelling_a_speech_job_removes_the_unimported_file` 验证取消清理。
- ai-vocals-tools：`src/crates/concat-api/src/vocals.rs:400`，`cache_job_reports_progress_finishes_with_two_stems_and_reuses_them`，以及 `:446` 的 `cancelling_removes_partial_stems`，开发汇报涵盖双音轨缓存、进度/轮询/取消。
- ai-enhance-tools：`src/crates/concat-api/src/enhance.rs:593`，`progress_and_cancel_use_the_existing_job_protocol`，以及 `:534` 的 `a_cached_copy_replaces_the_clip_in_headless_and_live_projects`，开发汇报涵盖进度/取消和成功替换。

以上是已有交接证据，本次未重跑，也不冒称独立的真实运行证据。主 Agent 已选 A 并将该口径写入任务 rev2：接受这些夹具证据，真实生命周期记为未覆盖。本报告据此交付；如需真实推理与生命周期验证，需要用户另行安装模型并另排复验，本次不下载模型。

## 问题分类

### 手册写错

无（本次已测路径）。ai-tools 明确区分 API available 和模型 installed，正确提示窗口安装入口；转写/配音/分离/增强的参数与实际错误相符。

### 工具行为与手册不符

无（本次已测路径）。只读查询没有 modelMissing 是合理行为；无转写时先报 notFound，并不证明已验证词级查询或区间过滤。

### 工具缺陷

无（本次已测路径）。缺模型造成的覆盖不足是环境限制，不作为工具缺陷。没有真实推理，不能排除成功路径的缺陷，因此没有可提交的缺陷复现步骤。

复现缺模型覆盖限制：按“四类模型状态”确认所有 installed=false → 按逐工具表中 4 个有效形状请求启动 AI 任务 → 分别返回 M1/M2/M3/M4、没有 job → 无法进行真实进度/取消状态验证。不要下载模型或写假权重来绕开。

## 清理与产物

- `project_close({"path":P})` 返回成功。
- 正常目录只读 headless PID 1513 关闭 stdin 后 exit 0；portable headless PID 1622 同样 exit 0。
- Python MCP 桥接进程已结束；Cargo 构建进程已 exit 0。没有遗留本 session 启动的应用、下载或推理进程。
- 整个 `/Users/elon/eloncut-ai-verify-zu0mccyt` 已删除，包括临时工程、clone executable、portable 配置；清理返回 `exists=false`，另行文件检查也确认目录不存在。
- 唯一新增仓库文件为本报告；没有修改业务代码、手册或其他会话文件。plan 维护的进度记录由 E-Flow 管理。
- 无 AI 成功产物可交付，未调用 preview_frame 冒充 AI 结果。工程原始文件位置为 `/Users/elon/eloncut-ai-verify-zu0mccyt/acceptance/concat.json`，现在随临时目录删除。
- 未覆盖：真实词级转写/时间码正确性、转写筛选、真实语音合成/入轨、音轨分离及听感、增强图像/片段、成功状态、真实进度与取消、window UI。四类模型项目均明确为**未做真实运行**。

## 实际工作树关键文件 SHA-256

下列文件在初次记录与验收后复核一致；路径相对仓库。此清单说明当前工作树验收基线，不覆盖提交外的修改归属。

| 文件 | SHA-256 |
| --- | --- |
| `src/crates/concat-api/src/lib.rs` | `d5c20e54b579ab022b268e03e46329c40b81c290d18400b666b135ccac6bf00c` |
| `src/crates/concat-api/src/speech.rs` | `a8fe68fe554b0c4d549f9a6d154132048bb5ab9c56b929c707f3445e41b25be0` |
| `src/crates/concat-api/src/transcription.rs` | `17b05142b1da67e52d7b331565b0b3b62e263f9cba4aeee2740d42f4adce1473` |
| `src/crates/concat-api/src/enhance.rs` | `74d75746abc1756f0539eb18535ffc9410d99f8780efae5176958bb84a58fd14` |
| `src/crates/concat-api/src/vocals.rs` | `693aa8d31695c9b2c2177e3aa86d156bb83ada55879bea27e6dda87bb04ee67e` |
| `src/crates/concat-speech/src/tts.rs` | `8565154409acaee15e50a8400503a3104b166f66b3e96da6e36b183033ff09e6` |
| `src/crates/concat-mcp/src/tools.rs` | `c1675169ebd6f15514460da482fd11f551cae2471ca22fb0079fed2ab78b8447` |
