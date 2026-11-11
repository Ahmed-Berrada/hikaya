# Windows development setup

This is the main Hikaya development and testing environment: a Windows 10/11 x64 machine with an
NVIDIA GPU. Inference uses whisper.cpp's **Vulkan** backend, which runs on any recent NVIDIA, AMD or
Intel driver. No CUDA toolkit is needed.

## 1. Install the toolchain

Run in **PowerShell** (as your normal user; winget asks for elevation when needed):

```powershell
winget install --id Git.Git -e
winget install --id GitHub.cli -e
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install --id Kitware.CMake -e
winget install --id KhronosGroup.VulkanSDK -e
winget install --id Rustlang.Rustup -e
winget install --id Oven-sh.Bun -e
```

Then open a **new** terminal (so `PATH` and `VULKAN_SDK` are picked up) and check:

```powershell
rustup default stable-msvc
rustc --version; cargo --version; bun --version; cmake --version; git --version
echo $env:VULKAN_SDK
nvidia-smi   # driver is installed and sees the GPU
```

WebView2 is preinstalled on Windows 10 (21H2+) and 11. Tauri uses it for the UI.

## 2. Clone and run

```powershell
gh auth login
gh repo clone Ahmed-Berrada/hikaya
cd hikaya
bun install
New-Item -ItemType Directory -Force src-tauri/resources/models | Out-Null
curl.exe -L -o src-tauri/resources/models/silero-vad-v6.2-F32.gguf https://huggingface.co/handy-computer/silero-vad-v6.2-gguf/resolve/main/silero-vad-v6.2-F32.gguf
bun run tauri dev
```

The first build compiles whisper.cpp and the Vulkan shaders, which takes several minutes. Later
builds are incremental.

## 3. Before every commit

```powershell
bun run lint
bun run format:check
cd src-tauri; cargo clippy --all-targets; cargo test; cd ..
```

Follow the branch, commit and PR rules in [`AGENTS.md`](../../AGENTS.md).

## 4. Local LLM (optional, for post-processing and summaries)

```powershell
winget install --id Ollama.Ollama -e
ollama pull qwen3:4b
```

Ollama listens on `http://127.0.0.1:11434` and uses the GPU automatically.

## Troubleshooting

See [BUILD.md](../../BUILD.md#troubleshooting). Path-length and Vulkan SDK issues are covered there.
