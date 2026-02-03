<h1 align="center">
  <img src="./assets/banner.jpg" alt="Cortex" width="100%">
</h1>

<h3 align="center">The Agent-Native Development CLI</h3>

<p align="center">
  Our agent is continuously <strong>fine-tuned</strong> through community-driven incentives, enabling contributors to collaborate and compete to produce the best possible AI agent.
</p>

<p align="center">
  <a href="https://github.com/CortexLM/cortex/releases"><img src="https://img.shields.io/github/v/release/CortexLM/cortex?style=flat-square&color=blue" alt="Release"></a>
  <a href="https://discord.gg/cortexfoundation"><img src="https://img.shields.io/badge/Discord-Join%20Us-5865F2?style=flat-square&logo=discord&logoColor=white" alt="Discord"></a>
  <a href="https://twitter.com/CortexLM"><img src="https://img.shields.io/twitter/follow/CortexLM?style=flat-square&logo=twitter&color=1DA1F2" alt="Twitter"></a>
</p>

<p align="center">
  <a href="#installation">Installation</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="https://discord.gg/cortexfoundation">Discord</a> •
  <a href="https://twitter.com/CortexLM">Twitter</a> •
  <a href="./CHANGELOG.md">Changelog</a>
</p>

---

## Installation

### Linux & macOS

```bash
curl -fsSL https://software.cortex.foundation/install.sh | sh
```

Or with wget:

```bash
wget -qO- https://software.cortex.foundation/install.sh | sh
```

### Windows (PowerShell)

```powershell
irm https://software.cortex.foundation/install.ps1 | iex
```

### Install a specific version

```bash
# Linux & macOS
CORTEX_VERSION=0.0.1c curl -fsSL https://software.cortex.foundation/install.sh | sh

# Windows PowerShell
$env:CORTEX_VERSION="0.0.1c"; irm https://software.cortex.foundation/install.ps1 | iex
```

### Manual Download

| Platform | Architecture | Download |
|----------|--------------|----------|
| **Linux** | x86_64 | [cortex.tar.gz](https://software.cortex.foundation/v1/assets/linux-x86_64/latest/cortex.tar.gz) |
| **Linux** | ARM64 | [cortex.tar.gz](https://software.cortex.foundation/v1/assets/linux-aarch64/latest/cortex.tar.gz) |
| **macOS** | Apple Silicon | [cortex.tar.gz](https://software.cortex.foundation/v1/assets/darwin-aarch64/latest/cortex.tar.gz) |
| **macOS** | Intel | [cortex.tar.gz](https://software.cortex.foundation/v1/assets/darwin-x86_64/latest/cortex.tar.gz) |
| **Windows** | x86_64 | [cortex.zip](https://software.cortex.foundation/v1/assets/windows-x86_64/latest/cortex.zip) |
| **Windows** | ARM64 | [cortex.zip](https://software.cortex.foundation/v1/assets/windows-aarch64/latest/cortex.zip) |

---

## Quick Start

```bash
# Start a new session
cortex

# Or run a specific command
cortex "explain this codebase"
```

---

## Update

Cortex can update itself:

```bash
cortex upgrade
```

Or reinstall using the installation script.

---

## Uninstall

### Linux & macOS

```bash
sudo rm /usr/local/bin/Cortex
# Or if installed to ~/.local/bin
rm ~/.local/bin/Cortex
```

### Windows

```powershell
Remove-Item "$env:LOCALAPPDATA\Cortex\Cortex.exe"
# Remove from PATH via System Properties > Environment Variables
```

---

## Contributing

We welcome contributions from the community! See our [Contributing Guide](./docs/CONTRIBUTING.md) for details.

---

## License

Copyright © 2025 Cortex Foundation. All rights reserved.
