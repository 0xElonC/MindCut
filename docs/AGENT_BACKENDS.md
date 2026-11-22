# Agent 多后端接入(Codex + Antigravity)

状态:阶段 0 至 3 已实现并通过测试,阶段 4(窗口里的真实场景检验)待做。2026-11 调研,`agy` 1.5.14。

## 实现与方案的差异(以代码为准)

- **注册表与描述符**在 `backend/mod.rs`(`REGISTRY`、`Descriptor`、`Caps`、`Backend`、`connect`)。Codex 在 `backend/codex/`,ACP 协议在 `backend/acp/`,Antigravity 只有 `backend/antigravity.rs` 一个文件。
- `AcpProfile` 的字段是 `arguments`、`system_prompt_meta`、`model_filter`、`tool_name`,没有 `SystemPrompt` 枚举(只有一种传法,需要第二种时再加)。
- **Antigravity 有深度选择器**:模型和 `thought_level` 是 `configOptions` 里的两个选项,见“阶段 0 验证结果”。
- **对话记录**由 `store::transcript::Recorder` 写,**按回合**落盘(回合结束时写,用户的话立即写),不是每个条目一行。它只关心已登记的对话,不管窗口当前显示哪一个,所以回合在别的对话上结束也不丢。
- 条目编号:ACP 的条目 id 以启动时间为起点递增,跨运行不重复,否则重开对话后新条目会和记录里的旧条目撞 id。
- 一致性测试在 `concat-agent/src/testing.rs`;Codex 和 ACP 各有一个假服务(内存管道,不起进程)。对真实程序的检查是 `#[ignore]` 测试,`cargo test -p concat-agent -- --ignored`;设置 `CONCAT_TEST_MCP` 为一个已构建的 `concat` 可再加一轮 MCP 工具调用。
- 窗口:`agent.rs` 里 `Slot`(每个后端一个)、`enter`、`ensure`、`take_up`、`remember`。后端选择器由 `REGISTRY` 生成;只有一个后端时不显示。

## 1. 需求

目标:Agent 面板除 Codex 外,可使用 Antigravity CLI(`agy`)的原生 Gemini 模型。

已定的决定:
- **后端按对话选择。** 新建对话时选后端,对话开始后锁定,恢复对话时回到原后端。两个后端的进程可同时存在。
- **ACP 后端必须能重建历史。** ElonCut 自己保存对话记录,不依赖 CLI 重放。
- 只用 `gemini-*` 模型,不展示 `agy` 里的 Claude 和 GPT-OSS。

范围外:Claude Code 后端、Gemini CLI(个人账号 2026-06-18 已停用)、ACP 后端的 skills、对话中途换后端。

非功能要求:
- Codex 现有行为和测试不变。
- 不改用户的 `~/.codex` 或 Antigravity 全局配置:MCP 和系统指令只对会话生效。
- 沿用现有安全模型(免审批、不开沙盒,Live 关闭仅靠提示词约束)。
- 26 个语言文件同步,否则测试失败。

## 2. 现有结构与改动落点

依赖方向:`concat`(窗口)→ `concat-agent`、`concat-server`、`concat-api`、`concat-host`。`concat-agent` 只依赖 serde_json 和 log,不认识窗口和视频,这一点要保持。

| 位置 | 现状 | 改动 |
|---|---|---|
| `concat-agent/src/rpc.rs`(489 行) | `Codex` 结构:进程、读线程、请求应答表、shim、probe | 改名 `peer.rs`,抽成通用 `Peer`;Codex 基于它 |
| `concat-agent/src/wire.rs`(742 行) | 请求构造、`Item`、`Update`、`Input`、`Model` 等 | 通用词汇移到 `vocab.rs`;Codex 专属部分移到 `backend/codex/wire.rs` |
| `concat-agent/src/store.rs`(184 行) | 对话元数据列表 `agent-chats.json` | 移到 `store/mod.rs`,`Chat` 加 `backend`;新增 `store/transcript.rs` |
| `concat-agent/src/locate.rs` | 只找 codex | 每个产品自带 `locate`(`backend/codex/locate.rs` 等),`CONCAT_CODEX`、`CONCAT_AGY` 覆盖 |
| `concat/src/agent.rs`(1365 行) | `Controller` 持有一个 `Arc<Codex>`,直接调 `thread/*` | 持有多个后端;调用改走 `Backend` trait,后端列表读 `REGISTRY` |
| `concat/ui/workspace/agent-pane.slint` | `AgentChat` 全局属性;`models`、`efforts`、`skills`、`status` | 加后端选择与能力属性;选项由 `REGISTRY` 生成,不写死 |
| `concat/src/live.rs`、`concat-server`、`concat-mcp` | MCP 经 socket 回到窗口 | **不改**,两个后端用同一个 `McpServer` 描述 |
| `concat/locales/*.json`(26 个) | `agent.*` 共 49 个键,其中 5 个左右写死 “Codex” | 加占位符 `{name}`,补新键 |
| `concat/src/lib.rs`、`dock.rs` | 调用 `agent::install`、`shutdown`、`attach` | 不改接口 |

数据位置:`AppDirs.config` 下已有 `agent-chats.json`。对话记录放 `AppDirs.config/agent-chats/<id>.jsonl`(小状态,和元数据在一处),不放项目文件夹。

## 3. 实测结论(`agy` 1.5.14)

- `agy acp --dangerously-skip-permissions`:stdio JSON-RPC。
- `session/new` 接受 `cwd`、`mcpServers`(command、args、env)、`_meta.antigravity.systemPrompt.append`。已用 ElonCut 自己的 MCP 跑通一次工具调用。
- 返回 `availableModels`:Gemini 3.8 Flash 的 high、medium、low 三档(深度编进模型名),另有 Claude 与 GPT-OSS。
- 事件:`agent_message_chunk`、`tool_call`、`tool_call_update`、`session_info_update`(标题)、`usage_update`;`session/prompt` 响应带 `stopReason`。
- **`session/load` 在新进程里能恢复模型上下文**(问“暗号”答对),并重放 `user_message_chunk` 和 `agent_message_chunk`。重放没有消息 ID,工具调用、思考和图片是否重放未验证,所以显示不能依赖它。
- 工具名带前缀 `mcp_eloncut_…`;`tool_call_update` 里没看到工具返回内容。
- 无 skills 列表接口;无重命名和归档方法。

### 阶段 0 验证结果(2026-11-22,`agy` 1.5.14,逐项实测)

| 项 | 结论 | 对设计的影响 |
|---|---|---|
| 模型与深度 | `session/new` 返回 `configOptions`:`model`(`gemini-3.8-flash` 等)和 `thought_level`(low/medium/high)是**分开的**。用 `session/set_config_option {configId, value}` 切换,已验证 `thought_level` 可切。`availableModels` 是旧写法,把深度编进了模型名 | **Antigravity 有深度选择器**(`Caps.efforts = true`);模型和深度都读 `configOptions`,模型按 `gemini-*` 过滤。不用 `availableModels` |
| 恢复时换环境 | `session/load` 在新进程里同时换了 `cwd`、`mcpServers` 和系统指令,三者都生效(新 MCP 的工具被调用,追加的系统指令被遵守) | `setup_on_resume = true`,**不需要**“消息前缀告知环境变化”的替代做法 |
| 停止 | `session/cancel` 通知 → 原 `session/prompt` 在 0.2 秒内返回 `stopReason: "cancelled"`;会话之后仍可继续对话 | 映射为 `TurnEnded(interrupted)` |
| 图片 | 只接受 `{"type":"image","data":"<base64>","mimeType":"image/png"}`;路径或 uri 返回 `-32602 data is required` | `send` 里读文件并 base64;注意大图要缩小 |
| 工具结果 | `tool_call_update` **没有任何输出内容**,成功和失败都只有 `status`(`completed`/`failed`)。`tool_call` 带 `toolCallId`、`name`、`rawInput` | `Item::Tool.output` 恒为空;卡片只显示状态和参数 |
| 思考 | 有 `agent_thought_chunk`(`content.text`),没有条目 id | 按回合内的思考段生成 id;映射为 `ThoughtWords` |
| 权限请求 | 不加 `--dangerously-skip-permissions` 时,Agent 发 `session/request_permission`,`params` 含 `sessionId`、`toolCall`、`options`(`{optionId:"allow",kind:"allow_once"}`、`{optionId:"deny",kind:"reject_once"}`)。回复 `{"outcome":{"outcome":"selected","optionId":"allow"}}` 有效 | 自动选 `kind` 含 `allow` 的第一项;仍加免审批参数 |
| 多会话 | 一个进程可同时有多个会话;`session/list`、`session/close` 可用 | 一个后端一个进程 |
| 重放 | `session/load` 重放 `user_message_chunk`、`agent_message_chunk`、`tool_call`(无结果)和 `session_info_update`,都在 `load` 响应之前到达 | 确认“恢复期间丢弃所有更新”的策略;显示只用自有记录 |
| 进程被杀 | 标准输出关闭,`Peer` 的读线程结束,在途请求按现有逻辑收到错误 | 复用 `Peer` 现有处理 |
| 模型切换 | `session/set_model` 返回成功;`session/set_config_option` 返回新的 `configOptions` | 统一用 `set_config_option` |

提示:测试中模型曾自行列出并打开本机的真实工程再关闭(`save:false`,`concat.json` 修改时间未变)。以后的探测都把 MCP 的 `HOME` 指到临时目录隔离。

## 4. 设计

### 4.1 分层:协议、产品、注册表(`concat-agent`)

设计原则:**“协议”和“产品”分开,新增后端只加文件、不改调用方。**
- 协议:怎么跟一个 CLI 说话(Codex app-server、ACP、将来可能有 Claude 的 stream-json)。一种协议写一次。
- 产品:某个具体 CLI 怎么启动、怎么找、怎么装、用哪些模型、有哪些能力。一个产品是一份**描述符**(数据),不是一份逻辑。
- 注册表:所有产品描述符的静态列表。界面的后端选择器、对话列表标记、“缺失”页、查找程序都从它生成,`agent.rs` 里**不允许出现 `match backend { Codex => …, Antigravity => … }`**,只允许读描述符和 `Caps`。

目录(阶段 1 一次定型,之后只增不改):

```
concat-agent/src/
  lib.rs
  peer.rs               通用 JSON-RPC 对端:启动命令、读写线程、请求应答表、stderr 尾部、
                        Windows shim、关闭通知。与具体协议无关
  vocab.rs              与后端无关的词汇:Item、Update、Input、Model、Skill、ToolState、ThreadSetup、McpServer
                        (现 wire.rs 的通用部分;窗口只认这些)
  backend/
    mod.rs              Backend trait、Caps、BackendId、Descriptor、Protocol、REGISTRY
    codex/              协议 + 产品:Codex app-server(现 wire.rs 的 Codex 部分、locate 的 codex)
      mod.rs  wire.rs  locate.rs
    acp/                协议:Agent Client Protocol 的通用客户端
      mod.rs            Backend 实现:握手、session/*、事件翻译、权限自动应答
      wire.rs           请求构造与事件解析(纯函数,可单测)
      profile.rs        AcpProfile:各产品在 ACP 上的差异(见下)
    antigravity.rs      产品:agy 的描述符 + AcpProfile
    (claude.rs)         将来:Claude Code 的描述符,见第 8 节
  store/
    mod.rs              对话元数据列表(现 store.rs),Chat 带 backend
    transcript.rs       对话记录(追加写 jsonl);所有 remote_history=false 的后端共用
  testing/              假服务与一致性测试(见 8.2),仅 cfg(test)
```

核心类型:

```rust
/// 注册表里的稳定标识,也是写进 agent-chats.json 的字符串。一旦发布不可改名。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct BackendId(pub &'static str);        // "codex"、"antigravity"、将来的 "claude"

/// 一个产品:纯数据。
pub struct Descriptor {
    pub id: BackendId,
    pub name: &'static str,                    // 界面上的名字(专有名词,不翻译)
    pub locate: fn() -> Option<PathBuf>,       // 找程序,含环境变量覆盖
    pub install: Install,                      // 缺失页怎么提示:命令、网址
    pub protocol: Protocol,
    pub caps: Caps,
}

pub enum Protocol {
    Codex,
    Acp(AcpProfile),
    // 将来:ClaudeStream(ClaudeProfile)
}

/// ACP 上各产品的差异,全部是数据或小函数。
pub struct AcpProfile {
    pub args: &'static [&'static str],         // 例:["acp", "--dangerously-skip-permissions"]
    pub system_prompt: SystemPrompt,           // Meta("antigravity.systemPrompt.append") | FirstMessage
    pub model_filter: fn(&str) -> bool,        // 例:|id| id.starts_with("gemini-")
    pub strip_tool_prefix: fn(&str) -> &str,   // "mcp_eloncut_x" → "x"
}

pub struct Caps {
    pub efforts: bool,            // 显示深度选择器
    pub skills: bool,
    pub remote_history: bool,     // 历史由 CLI 保管(Codex);false 则用自有记录(transcript)
    pub rename: bool,
    pub setup_on_resume: bool,    // 恢复时能换 cwd、指令、MCP(Codex 与 Antigravity 都为 true)
}

pub trait Backend: Send + Sync {
    fn id(&self) -> BackendId;
    fn start(&self, setup: &ThreadSetup, done: Done<String>);               // 返回会话 id
    fn resume(&self, id: &str, setup: &ThreadSetup, done: Done<Vec<Item>>); // remote_history=false 的实现返回空
    fn send(&self, id: &str, inputs: &[Input], model: Option<&str>, effort: Option<&str>, done: Done<String>);
    fn interrupt(&self, id: &str, turn: &str);
    fn rename(&self, id: &str, title: &str);
    fn archive(&self, id: &str);
    fn models(&self) -> Result<Models, String>;
    fn skills(&self, cwd: &str) -> Vec<Skill>;
}

/// 按描述符造出后端:启动进程并握手。窗口只调用这一个函数。
pub fn connect(d: &Descriptor, on_event: impl Fn(Update) + Send + 'static) -> Result<Arc<dyn Backend>, String>;
pub static REGISTRY: &[&Descriptor] = &[&codex::DESCRIPTOR, &antigravity::DESCRIPTOR];
```

事件统一翻译成 `vocab.rs` 的 `Update`。`agent.rs` 的 `heard`、`update`、`put_item` 因此基本不变。

**约束(写进模块注释,评审时核对):**
1. `concat-agent` 不依赖任何窗口、视频、`concat-*` crate。
2. 窗口代码(`agent.rs`、Slint)不得出现具体后端的名字,只能读 `Descriptor` 与 `Caps`。
3. 协议模块不得出现具体产品的名字;产品差异只能放在 `Profile` 里。
4. 能力用 `Caps` 表达,不用“是不是某后端”来判断。

### 4.2 ACP 事件映射

| ACP | `Update` |
|---|---|
| `session/prompt` 发出,响应 | `TurnStarted`,`TurnEnded`(`end_turn`→completed,`cancelled`→interrupted,错误→failed) |
| `agent_message_chunk` | `AgentWords`(item id 由本端按回合生成) |
| `agent_thought_chunk` | `ThoughtWords`(已验证存在;无条目 id,按回合生成) |
| `tool_call`、`tool_call_update` | `ItemStarted`、`ItemDone`(`toolCallId` 为 id;`status` 为 completed 或 failed);名称去掉 `mcp_<server>_` 前缀;摘要取 `rawInput` 并去掉 `toolSummary`;无输出内容 |
| `session_info_update` | `Named` |
| `session/request_permission` | 自动选允许 |
| JSON-RPC error | `Trouble` |

### 4.3 历史重建(ACP)

ElonCut 保存每个 ACP 对话的记录,显示只读这份记录:
- 文件 `agent-chats/<id>.jsonl`,每行一个 `Item`(用户、助手、思考、工具、通知)。追加写入,每个条目完成时写一行,中途崩溃最多丢最后一条。
- `Item` 需要可序列化。`concat-agent` 现在只依赖 `serde_json`,用手写的 `to_json`/`from_json`(和 `store.rs` 风格一致),不新增 `serde` 依赖。
- 用户消息里的图片只存路径。文件被移动后显示为缺失,和 Codex 现状一致。
- **恢复流程:** 先读记录并立刻显示;同时 `session/load` 恢复模型上下文;`load` 期间收到的所有 `session/update`(重放内容)一律丢弃,否则会重复。
- 记录丢失或损坏:显示提示“记录不可用,模型上下文仍在”,可继续对话,不阻止。
- Codex 对话保持现状(`remote_history = true`,历史来自 `thread/resume`)。以后若要统一,把 Codex 也迁到自有记录,但不在这一期。
- 删除对话:删元数据、删记录文件,ACP 另发 `session/close`。

### 4.4 按对话选后端(`agent.rs`)

`Controller` 的状态拆成两层:

```
Controller
  backends: HashMap<BackendId, Slot>      // Slot { link: Link, backend: Option<Arc<dyn Backend>>, models, defaults }
  current: Option<String>            // 当前会话 id
  current_backend: BackendId                 // 当前对话的后端;新对话取“上次所用”
  loaded: HashMap<String, String>    // 保持
  live / bridge                      // 共享一个 MCP 桥,两个后端共用
```

- 后端进程**按需启动**:第一次有对话要用它时才 `begin(id)`。启动时只起默认那个,不为没用的后端拉进程。
- `Link`(Missing、Failed、Starting、Ready)按后端各一份;状态页显示**当前对话后端**的状态。Codex 缺失不影响在 Antigravity 里对话,反过来也一样。
- 新对话页:显示后端选择器,可切换(切换即换 `current_backend` 并按需启动)。发出第一条消息后,对话的 `backend` 写进 `Chat`,选择器变成只读标签。
- 打开旧对话:`current_backend = chat.backend`,按需启动对应后端后 `resume`。
- 事件路由:`Peer` 回调带 `BackendId` 一起 `post`。会话 id 在两个后端间不会冲突(都是 UUID),`ours(thread)` 的判断照旧。
- 停止、模型、深度:都按当前后端的 Slot 取。
- 关闭窗口:`shutdown` 停掉所有后端进程和桥。
- `instructions()`、MCP 桥、附件、Live 逻辑复用不变。`ThreadSetup` 保持,各后端按自己的协议翻译。

新对话的后端取“上一次用的”:发出消息时记进 `prefs.rs` 的 `Preferences`(`agent_backend`),没有记录时为 Codex,不改变老用户的行为。设置页不加选项。

### 4.5 界面

`AgentChat` 新增:
- `backends: [string]`、`backend-index`、`backend-locked: bool`、`callback backend-changed(int)`:沿用 `Select` 组件,和模型选择器同排。
- `has-efforts: bool`、`has-skills: bool`:由 `Caps` 驱动,隐藏深度选择器和魔法棒。
- `AgentSession` 增加 `backend: string`,列表里每条对话显示一个小标记。
- 状态页(`missing`、`failed`、`connecting`)文案带后端名。Missing 页的按钮:Codex 打开 OpenAI 页面;Antigravity 显示安装命令 `curl -fsSL https://antigravity.google/cli/install.sh | bash` 并可打开官网。`on_open_help` 要按后端选链接。

文案:`agent.connecting`、`agent.connectingBody`、`agent.failedTitle`、`agent.missingBody`、`agent.missingTitle` 改成带 `{name}` 的版本,新增后端名、标记、安装说明等键。26 个语言文件全部同步。

### 4.6 ACP 后端细节

- 握手:`initialize`,协议版本 1。
- 模型与深度:`session/new` 返回的 `configOptions` 里取 `id == "model"` 与 `id == "thought_level"`。模型按 `AcpProfile.model_filter` 过滤(Antigravity:`gemini-*`)。启动后用一个临时会话读取,读完 `session/close`。
- 切换:`session/set_config_option {sessionId, configId, value}`,在 `send` 前按需调用(与当前值不同才调用)。
- 图片:读文件,编码 base64,按扩展名给 `mimeType`;过大的图先缩小(阶段 2 定上限)。
- 停止:`session/cancel` 通知;回合在原 `session/prompt` 响应里以 `cancelled` 结束。
- 系统指令:新建与恢复都用 `_meta.antigravity.systemPrompt.append`。项目或 Live 状态变化时,恢复会话时直接传新的 `cwd`、`mcpServers` 和指令。
- 权限请求:自动选 `kind` 含 `allow` 的第一项;其他 Agent 发来的请求回空结果。
- 指令里要强调编辑视频只用 `eloncut` 的工具:`agy` 自带浏览器、`run_command`、`write_to_file` 等很多工具,实测中模型会在 MCP 之外另用 shell。

## 5. 实施阶段

**阶段 0:验证未知项(已完成,结果见上表)。** 只写脚本和记录,不改产品代码。需要确认:
1. `session/load` 时能否换 `cwd`、`mcpServers`、系统指令。
2. `session/cancel` 与停止后的 `stopReason`。
3. 图片块的格式。
4. 工具结果在哪个字段、成功与失败各是什么样。
5. 是否有 `agent_thought_chunk`。
6. 权限请求的格式(让它跑 shell 命令触发)。
7. 一个进程服务多个会话;崩溃、未登录时的报错形态。
8. 模型切换方法;用临时会话取模型列表是否可行。
9. `load` 重放的内容范围(工具、思考),只用于确认“丢弃”策略足够。

产出:验证记录追加到本文档;一个可复用的 ACP 假服务脚本,供阶段 2 的集成测试。

**阶段 1:抽象,不改行为。** 单独提交,和后面的功能分开。
1. 目录定型(见 4.1):`rpc.rs` → `peer.rs` 并抽出通用 `Peer`;`wire.rs` 的通用部分 → `vocab.rs`;Codex 部分 → `backend/codex/`;`store.rs` → `store/mod.rs`。纯移动和改名,不改逻辑,单独一个提交。
2. 定义 `BackendId`、`Descriptor`、`Protocol`、`Caps`、`Backend`、`REGISTRY`、`connect`,把 Codex 做成第一个描述符。
3. `agent.rs` 改走 trait 与注册表,但仍只有 Codex 一个后端。
4. 写 `testing/` 的一致性测试,让 Codex 先通过(用它验证抽象没有漏)。
验收:`cargo test`、clippy 通过;手工跑一轮 Codex 对话(含恢复、停止、删除)与改动前一致。

**阶段 2:ACP 后端和记录。**
1. `backend/acp/`:请求构造、事件解析、自动应答,配单元测试(风格同 `codex/wire.rs`);`backend/antigravity.rs` 只含描述符与 `AcpProfile`。
2. `store/transcript.rs`:追加、读取、损坏容错,配单元测试。
3. 用 ACP 假服务通过 `conformance`(见 8.2)。
验收:对真实 `agy` 完成一轮带 MCP 工具调用的对话;重启后记录完整、上下文仍在。

**阶段 3:控制器、界面、设置、文案。**
1. `Controller` 拆成多个 Slot(以 `BackendId` 为键),按需启动,事件带 `BackendId`。
2. `Chat` 加 `backend`(旧文件缺字段时默认 `codex`)。
3. Slint:后端选择器、`Caps` 驱动的显示、列表标记、状态页。
4. `prefs.rs`:记住上一次用的后端(`agent_backend`)。
5. 26 个语言文件。
验收:两个后端在窗口里都能用;同一窗口里两个后端的对话来回切换;一个后端缺失时另一个不受影响;旧的 `agent-chats.json` 能直接读。

**阶段 4:真实场景与文档。**
- 用 Antigravity 在 Live 模式完成“加标题并导出”;测附件、停止、崩溃恢复、关窗口时进程退出。
- 更新 README 的 Agent 一节与 `docs/MCP.md`。

## 6. 风险

| 风险 | 处理 |
|---|---|
| ACP 工具调用没有输出内容 | 卡片只显示状态与参数,不显示空的输出区 |
| 重放与自有记录重复 | 恢复期间丢弃全部 `session/update`,恢复完成后才接收新事件 |
| 模型优先用 `agy` 自带工具而绕过 MCP | 指令明确;阶段 4 实测;必要时在指令里列出禁用项 |
| `agy` 闭源且迭代快,事件格式可能变 | 未知事件忽略;日志记录 `agy` 版本 |
| 工具输出缺失 | 卡片折叠区不显示空内容 |
| 同时存在两个后端进程,退出时漏杀 | `shutdown` 遍历所有 Slot;`Peer` 的 `Drop` 已会 kill |
| 阶段 1 动到稳定路径 | 单独提交,靠现有测试加手工验证 |
| Live 关闭只靠提示词 | 与 Codex 相同,在文档中说明 |

## 7. 已确认

- 新对话的默认后端只记住上一次用的,不加设置项(`prefs.rs` 里存一个 `agent_backend` 字段即可,不进设置页)。
- (原“`session/load` 不能换环境”的备选方案已不需要:阶段 0 验证它能换。)

## 8. 新增一个后端的步骤(给后来者)

先判断新 CLI 属于哪种情况:

| 情况 | 例子 | 要做的事 |
|---|---|---|
| 它说 ACP | Antigravity;Claude Code 经第三方 ACP 适配器 | 写一份描述符 + `AcpProfile`,约 50 行,不写协议代码 |
| 它有自己的协议 | Codex;Claude Code 原生 stream-json | 新增一个协议模块(类似 `codex/`),再写描述符 |

清单(按顺序,每步可单独提交):
1. **探测。** 在本机逐项验证并把结果追加到本文档第 3 节:启动命令、注入 MCP 的方式、追加系统指令的方式、免审批参数、模型列表来源、停止方式、会话恢复、图片输入、工具结果字段、未登录和崩溃的报错形态。可复用阶段 0 的探测脚本。
2. **写协议模块**(仅当不是 ACP)。请求构造和事件解析写成纯函数,放在 `backend/<名>/wire.rs`,配单元测试,风格同 `codex/wire.rs`。事件翻译成 `vocab.rs` 的 `Update`,不要为新后端新增 `Update` 变体;如果确实缺一种,先在 `vocab.rs` 加,并让现有后端也能表达。
3. **写描述符**(`backend/<名>.rs`):`id`、`name`、`locate`(含 `CONCAT_<名>` 环境变量覆盖,指定后找不到就不回退)、`install`、`protocol`、`caps`。
4. **注册。** 在 `REGISTRY` 加一行。选择器、对话标记、缺失页自动出现,不需要改 `agent.rs` 或 Slint。
5. **文案。** `agent.*` 里和后端有关的键已经带 `{name}` 占位符,通常无需新增键;`install` 的说明若需翻译,新增键并同步全部 26 个语言文件。
6. **一致性测试。** 在 `testing/` 加一个假服务(脚本或小程序),让它通过 8.2 的一致性用例。
7. **真机检验。** 对真实 CLI 完成:一轮带 MCP 工具调用的对话、停止、恢复对话、Live 模式加标题并导出、CLI 崩溃后的恢复提示。
8. **文档。** README 的 Agent 一节加一行;本文档第 3 节记录该 CLI 的实测结论与版本。

### 8.1 Claude Code 的预研(未实现,仅供接入时参考)

本机已有 Claude Code 2.1.329,`--help` 里可见:`-p`、`--input-format`、`--output-format stream-json`、`--include-partial-messages`、`--mcp-config`、`--resume`、`--append-system-prompt`、`--dangerously-skip-permissions`、`--model`、`--effort`。以下是依据 `--help` 的判断,尚未实际跑过:

| 路线 | 做法 | 取舍 |
|---|---|---|
| A. 原生 stream-json | 新增 `backend/claude/` 协议模块:常驻 `claude -p --input-format stream-json --output-format stream-json --include-partial-messages`,`--mcp-config` 传 `eloncut`,`--append-system-prompt` 传指令,`--resume <id>` 恢复 | 不依赖第三方;要写一个协议模块;事件格式要自己解析 |
| B. ACP 适配器 | 描述符 + `AcpProfile`,启动 Zed 的 Claude ACP 适配器(npm 包) | 代码最少;多一层第三方依赖,要核对它支持的能力 |

建议:先按 A 验证一次,因为它不需要用户额外安装东西。`Caps` 预计:`efforts: true`、`skills: false`、`remote_history: false`(用 ElonCut 自己的记录,不去解析 `~/.claude` 下的文件)、`rename: false`。

### 8.2 一致性测试(所有后端共用)

`testing/` 提供一个函数 `conformance(descriptor, fake_server)`,每个后端都必须通过:
1. 启动并握手后,`models()` 返回非空。
2. `start` → `send` 一条消息 → 依次收到 `TurnStarted`、至少一个 `AgentWords`、`TurnEnded(completed)`。
3. 一次工具调用产生 `ItemStarted` 与 `ItemDone`,名称已去掉产品前缀。
4. `interrupt` 后收到 `TurnEnded(interrupted)`。
5. 进程中途退出:在途请求收到错误,并收到一次 `Closed`。
6. 未知事件被忽略,不崩溃。
7. 如果 `remote_history = false`:`transcript` 追加、重读、损坏行容错。
这样新增后端是否合格,有一份固定的判据。
