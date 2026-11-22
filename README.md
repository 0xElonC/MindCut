<p align="center">
  <img src="assets/brand/mindcut-logo.svg" alt="MindCut" width="128" />
</p>

<h1 align="center">MindCut</h1>
<h3 align="center">A free, local video editor you can talk to.</h3>

<p align="center">
  <b>English</b> · <a href="#简体中文">简体中文</a>
</p>

## About

MindCut is a free, open-source video editor for macOS, Windows and Linux, with an **Agent pane**: a chat docked beside the timeline where you describe an edit, attach footage or reference photos, and an agent cuts it for you, on the project you have open, while you watch.

The editor underneath is complete on its own: multi-track timeline, keyframes, effects, titles, auto-captions, text-to-speech, background removal, 4K export, no watermark, no account. Everything runs on your machine.

## The Agent pane

The agent is the **[Codex](https://developers.openai.com/codex/cli) already installed on your computer**. MindCut uses its models, its reasoning depth and its sign-in as they are in `~/.codex/config.toml`; nothing is copied into the app and no key is asked for.

- **Chat that edits.** The agent edits through the built-in [MCP server](docs/MCP.md): cut, trim, move, titles, captions, effects, keyframes, export, and a look at any frame to check its work.
- **Live mode.** With **Live** on, the agent edits the project open in the window. Each edit is drawn at once, joins the window's history and is undone with the same Undo. With it off, the agent never touches the open project.
- **Photos, videos and files.** Attach with the paperclip, or drag files onto the window after clicking in the message box. Photos are shown to the model; videos and other files are handed over as paths, to be imported with the tools.
- **Skills.** The wand button beside the paperclip lists the skills your Codex has, with a search. Pick one or more and they go with the message as pills above the message box.
- **Model and depth.** Pick the model and the reasoning depth beside the message box. The first choice is whatever your own Codex settings use.
- **Conversations.** Start, search, rename, delete and resume conversations from the clock button. A conversation comes back with its whole history.
- **Free to act.** The agent runs without approval prompts and its commands are not sandboxed: it can do on this computer what you can. Turn Live off and read what it does if that is not what you want.

Add the pane from the dropdown at the corner of any panel, like the Library or the Timeline.

**Needs:** Codex installed and signed in (`npm i -g @openai/codex`, or the Codex app). Without it the pane says so and offers to check again.

## Quick start

From a fresh checkout, one command installs what is missing, builds and starts MindCut:

```sh
./run.sh        # macOS
```

```bat
run.cmd         # Windows
```

It asks before installing anything and changes nothing that is already in place. `--yes` (`-Yes` on Windows) skips the questions, `--setup-only` (`-SetupOnly`) stops after the setup, and `--app` (`-App`) builds the shipping profile. The first build takes a while; later ones take seconds.

- **macOS:** Xcode command-line tools, Homebrew, FFmpeg 7+, cmake, pkgconf, Rust at the pinned version, and Codex through npm.
- **Windows 10/11:** through `winget`: Visual Studio C++ build tools, Rust, cmake and LLVM. FFmpeg's libraries are downloaded and checked against `scripts/ci/deps.lock` into `src/vendor/ffmpeg/win64`. Codex is installed through npm (and Node.js) if you agree. The Windows script has not been run on a real Windows machine yet; if a step fails, the message says which.
- **Codex is optional.** Without it the editor works and the Agent pane says Codex is missing. Sign in once with `codex login`.

## Build from source

You need Rust 1.93 or newer (see `rust-version` in `src/Cargo.toml`), the FFmpeg 7+ development libraries, cmake and a C++ compiler.

- **macOS:** `brew install ffmpeg`
- **Windows:** from [FFmpeg's Windows downloads](https://ffmpeg.org/download.html#build-windows), take BtbN's `win64-gpl-shared` build, unpack it and point `FFMPEG_DIR` at it
- **Linux:** use your distro's FFmpeg if it is version 7 or newer, otherwise a BtbN `shared` build with `FFMPEG_DIR` like on Windows

```sh
cd src
cargo run --profile quick -p concat    # the editor window
cargo test                             # the test suite
```

If you change text the interface shows, keep every file in `src/crates/concat/locales/` in step: a test fails when a language lacks a key. More in [CONTRIBUTING.md](CONTRIBUTING.md).

## Where it comes from

MindCut is a fork of **[Concat](https://github.com/jub0t/Concat)** by Jareer Abdullah and its contributors. The editor, the engine and the MCP server are theirs; MindCut adds the Agent pane (`src/crates/concat-agent`, `src/crates/concat/src/agent.rs`, `live.rs`), the live bridge in the API (`concat_api::Live`) and the new name.

Inside the code the name is left as it was: the crates are `concat-*`, the program is `concat`, and the project format (`concat.json`) is unchanged, so projects open in both. The upstream remote is kept as `upstream` for merging its fixes. The Concat name and marks belong to its authors ([TRADEMARK.md](TRADEMARK.md)); the original README is kept as [docs/README.upstream.md](docs/README.upstream.md), and its translations in `docs/` describe Concat.

## Licence

MindCut is free software under [AGPL-3.0-or-later](LICENSE), as Concat is. If you run a modified version for others over a network, you must offer them its source. See [LICENSING.md](LICENSING.md) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

---

<a id="简体中文"></a>

## 简体中文

**MindCut** 是一款免费开源、完全本地运行的视频编辑器(macOS、Windows、Linux),自带 **Agent 面板**:在时间线旁边直接对话,描述想要的剪辑,附上素材或参考图,智能体就在你打开的工程上动手剪,你能看着它剪。

**智能体用的是你电脑上已安装的 [Codex](https://developers.openai.com/codex/cli)**,模型、推理深度和登录都沿用 `~/.codex/config.toml`,不需要再填密钥。

- **能动手的对话**:通过内置 [MCP 服务](docs/MCP.md)剪切、裁剪、移动、加标题和字幕、特效、关键帧、导出,还会取帧检查自己的成果。
- **实时模式(Live)**:开启后智能体直接编辑当前打开的工程,每一步都立刻显示,计入窗口的撤销历史;关闭后它不会碰你打开的工程。
- **照片、视频、文件**:点回形针添加,或在消息框里点一下后把文件拖进窗口。照片会直接给模型看,视频和其他文件以路径交给它,由它用工具导入。
- **技能(Skill)**:回形针旁的魔法棒按钮会列出你 Codex 里的技能,可搜索;选中的技能以标签形式随消息一起发送。
- **模型与推理深度**:在消息框旁选择,默认就是你自己 Codex 配置里的那个。
- **对话管理**:点时钟按钮新建、搜索、重命名、删除、恢复对话,恢复后保留完整历史。
- **可自由执行**:智能体不会弹审批,命令也没有沙盒,它能在这台电脑上做你能做的事。不放心就关掉 Live 并留意它的操作。

在任意面板右上角的下拉菜单里添加「智能体」面板。**前提**:已安装并登录 Codex(`npm i -g @openai/codex` 或 Codex 应用)。

**一键启动**:拉下仓库后,macOS 运行 `./run.sh`,Windows 运行 `run.cmd`,会检查并安装缺少的依赖(Rust、FFmpeg、cmake、Windows 上还有 C++ 构建工具和 LLVM、可选的 Codex),然后编译并启动。安装前会询问;`--yes`(Windows 为 `-Yes`)跳过询问,`--setup-only`(`-SetupOnly`)只装依赖不启动。Windows 脚本还没有在真实的 Windows 机器上跑过。

**来源**:MindCut 是 **[Concat](https://github.com/jub0t/Concat)**(作者 Jareer Abdullah 及贡献者)的分支,编辑器、引擎和 MCP 服务出自上游,遵循 AGPL-3.0-or-later。MindCut 增加了 Agent 面板、实时桥接和新名称;代码内部的 crate 名、`concat` 程序名和 `concat.json` 工程格式保持不变,工程两边通用。上游仓库保留为 `upstream` 远程。
