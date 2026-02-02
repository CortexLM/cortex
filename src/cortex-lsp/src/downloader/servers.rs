//! Predefined downloadable server configurations.

use super::types::{DownloadableServer, InstallMethod};

/// Gopls (Go language server).
pub fn gopls() -> DownloadableServer {
    DownloadableServer {
        id: "gopls".to_string(),
        name: "gopls".to_string(),
        github_repo: "golang/tools".to_string(),
        binary_pattern: "gopls{ext}".to_string(),
        asset_pattern: "gopls.{os}-{arch}*".to_string(),
        is_archive: false,
        archive_binary_path: None,
        install_method: None,
    }
}

/// Rust Analyzer (Rust language server).
pub fn rust_analyzer() -> DownloadableServer {
    let os_arch = if cfg!(target_os = "windows") {
        "x86_64-pc-windows-msvc"
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "aarch64-apple-darwin"
        } else {
            "x86_64-apple-darwin"
        }
    } else {
        if cfg!(target_arch = "aarch64") {
            "aarch64-unknown-linux-gnu"
        } else {
            "x86_64-unknown-linux-gnu"
        }
    };

    DownloadableServer {
        id: "rust-analyzer".to_string(),
        name: "Rust Analyzer".to_string(),
        github_repo: "rust-lang/rust-analyzer".to_string(),
        binary_pattern: "rust-analyzer{ext}".to_string(),
        asset_pattern: format!("rust-analyzer-{}.gz", os_arch),
        is_archive: false, // It's a gzip, but we handle it specially
        archive_binary_path: None,
        install_method: None,
    }
}

/// Pyright (Python language server).
pub fn pyright() -> DownloadableServer {
    DownloadableServer {
        id: "pyright".to_string(),
        name: "Pyright".to_string(),
        github_repo: "microsoft/pyright".to_string(),
        binary_pattern: "pyright-langserver{ext}".to_string(),
        asset_pattern: "pyright-*.tgz".to_string(),
        is_archive: true,
        archive_binary_path: Some("package/dist/pyright-langserver.js".to_string()),
        install_method: None,
    }
}

/// TypeScript Language Server.
pub fn typescript_language_server() -> DownloadableServer {
    DownloadableServer {
        id: "typescript-language-server".to_string(),
        name: "TypeScript Language Server".to_string(),
        github_repo: "typescript-language-server/typescript-language-server".to_string(),
        binary_pattern: "typescript-language-server{ext}".to_string(),
        asset_pattern: "typescript-language-server-*.tgz".to_string(),
        is_archive: true,
        archive_binary_path: Some("package/lib/cli.mjs".to_string()),
        install_method: None,
    }
}

/// Vue Language Server (Volar).
pub fn vue_language_server() -> DownloadableServer {
    DownloadableServer {
        id: "vue-language-server".to_string(),
        name: "Vue Language Server (Volar)".to_string(),
        github_repo: "vuejs/language-tools".to_string(),
        binary_pattern: "vue-language-server{ext}".to_string(),
        asset_pattern: "vue-language-server-*.tgz".to_string(),
        is_archive: true,
        archive_binary_path: Some("package/bin/vue-language-server.js".to_string()),
        install_method: None,
    }
}

/// Svelte Language Server.
pub fn svelte_language_server() -> DownloadableServer {
    DownloadableServer {
        id: "svelte-language-server".to_string(),
        name: "Svelte Language Server".to_string(),
        github_repo: "sveltejs/language-tools".to_string(),
        binary_pattern: "svelteserver{ext}".to_string(),
        asset_pattern: "svelte-language-server-*.tgz".to_string(),
        is_archive: true,
        archive_binary_path: Some("package/bin/server.js".to_string()),
        install_method: None,
    }
}

/// Biome (JavaScript/TypeScript linter and formatter with LSP).
pub fn biome() -> DownloadableServer {
    let os_arch = if cfg!(target_os = "windows") {
        "win32-x64"
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "darwin-arm64"
        } else {
            "darwin-x64"
        }
    } else {
        if cfg!(target_arch = "aarch64") {
            "linux-arm64"
        } else {
            "linux-x64"
        }
    };

    DownloadableServer {
        id: "biome".to_string(),
        name: "Biome".to_string(),
        github_repo: "biomejs/biome".to_string(),
        binary_pattern: "biome{ext}".to_string(),
        asset_pattern: format!("biome-{}.zip", os_arch),
        is_archive: true,
        archive_binary_path: Some("biome".to_string()),
        install_method: None,
    }
}

// ============================================================
// NEW SERVERS
// ============================================================

/// ZLS (Zig Language Server).
pub fn zls() -> DownloadableServer {
    let os_arch = if cfg!(target_os = "windows") {
        "x86_64-windows"
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "aarch64-macos"
        } else {
            "x86_64-macos"
        }
    } else {
        if cfg!(target_arch = "aarch64") {
            "aarch64-linux"
        } else {
            "x86_64-linux"
        }
    };

    DownloadableServer {
        id: "zls".to_string(),
        name: "ZLS (Zig Language Server)".to_string(),
        github_repo: "zigtools/zls".to_string(),
        binary_pattern: "zls{ext}".to_string(),
        asset_pattern: format!("zls-{}.tar.xz", os_arch),
        is_archive: true,
        archive_binary_path: Some("zls".to_string()),
        install_method: None,
    }
}

/// Clangd (C/C++ Language Server).
pub fn clangd() -> DownloadableServer {
    let os_arch = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else {
        "linux"
    };

    DownloadableServer {
        id: "clangd".to_string(),
        name: "clangd (C/C++ Language Server)".to_string(),
        github_repo: "clangd/clangd".to_string(),
        binary_pattern: "clangd{ext}".to_string(),
        asset_pattern: format!("clangd-{}-*.zip", os_arch),
        is_archive: true,
        archive_binary_path: Some(if cfg!(target_os = "windows") {
            "clangd_*/bin/clangd.exe".to_string()
        } else {
            "clangd_*/bin/clangd".to_string()
        }),
        install_method: None,
    }
}

/// Lua Language Server.
pub fn lua_language_server() -> DownloadableServer {
    let os_arch = if cfg!(target_os = "windows") {
        "win32-x64"
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "darwin-arm64"
        } else {
            "darwin-x64"
        }
    } else {
        if cfg!(target_arch = "aarch64") {
            "linux-arm64"
        } else {
            "linux-x64"
        }
    };

    DownloadableServer {
        id: "lua-language-server".to_string(),
        name: "Lua Language Server".to_string(),
        github_repo: "LuaLS/lua-language-server".to_string(),
        binary_pattern: if cfg!(target_os = "windows") {
            "lua-language-server.exe".to_string()
        } else {
            "lua-language-server".to_string()
        },
        asset_pattern: format!("lua-language-server-*-{}.tar.gz", os_arch),
        is_archive: true,
        archive_binary_path: Some("bin/lua-language-server".to_string()),
        install_method: None,
    }
}

/// Elixir LS (Elixir Language Server).
/// Note: Requires Elixir/Erlang OTP runtime to be installed.
pub fn elixir_ls() -> DownloadableServer {
    DownloadableServer {
        id: "elixir-ls".to_string(),
        name: "ElixirLS".to_string(),
        github_repo: "elixir-lsp/elixir-ls".to_string(),
        binary_pattern: if cfg!(target_os = "windows") {
            "language_server.bat".to_string()
        } else {
            "language_server.sh".to_string()
        },
        asset_pattern: "elixir-ls-*.zip".to_string(),
        is_archive: true,
        archive_binary_path: Some(if cfg!(target_os = "windows") {
            "language_server.bat".to_string()
        } else {
            "language_server.sh".to_string()
        }),
        install_method: None,
    }
}

/// Eclipse JDT Language Server (Java).
pub fn jdtls() -> DownloadableServer {
    let os = if cfg!(target_os = "windows") {
        "win32"
    } else if cfg!(target_os = "macos") {
        "macosx"
    } else {
        "linux"
    };

    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };

    // JDTLS uses a custom URL from Eclipse downloads
    DownloadableServer {
        id: "jdtls".to_string(),
        name: "Eclipse JDT Language Server (Java)".to_string(),
        github_repo: "eclipse-jdtls/eclipse.jdt.ls".to_string(),
        binary_pattern: if cfg!(target_os = "windows") {
            "jdtls.bat".to_string()
        } else {
            "jdtls".to_string()
        },
        asset_pattern: format!("jdt-language-server-*-{}-{}.tar.gz", os, arch),
        is_archive: true,
        archive_binary_path: Some("bin/jdtls".to_string()),
        install_method: Some(InstallMethod::CustomUrl {
            url_pattern:
                "https://www.eclipse.org/downloads/download.php?file=/jdtls/snapshots/jdt-language-server-latest.tar.gz&r=1"
                    .to_string(),
            is_archive: true,
            archive_binary_path: Some("bin/jdtls".to_string()),
        }),
    }
}

/// Terraform Language Server.
pub fn terraform_ls() -> DownloadableServer {
    let os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };

    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
    };

    DownloadableServer {
        id: "terraform-ls".to_string(),
        name: "Terraform Language Server".to_string(),
        github_repo: "hashicorp/terraform-ls".to_string(),
        binary_pattern: "terraform-ls{ext}".to_string(),
        asset_pattern: format!("terraform-ls_*_{}_{}.zip", os, arch),
        is_archive: true,
        archive_binary_path: Some("terraform-ls".to_string()),
        install_method: None,
    }
}

/// YAML Language Server (npm).
pub fn yaml_language_server() -> DownloadableServer {
    DownloadableServer {
        id: "yaml-language-server".to_string(),
        name: "YAML Language Server".to_string(),
        github_repo: "redhat-developer/yaml-language-server".to_string(),
        binary_pattern: "yaml-language-server{ext}".to_string(),
        asset_pattern: String::new(), // Not used for npm install
        is_archive: false,
        archive_binary_path: None,
        install_method: Some(InstallMethod::Npm {
            package: "yaml-language-server".to_string(),
            binary_name: "yaml-language-server".to_string(),
        }),
    }
}

/// Bash Language Server (npm).
pub fn bash_language_server() -> DownloadableServer {
    DownloadableServer {
        id: "bash-language-server".to_string(),
        name: "Bash Language Server".to_string(),
        github_repo: "bash-lsp/bash-language-server".to_string(),
        binary_pattern: "bash-language-server{ext}".to_string(),
        asset_pattern: String::new(), // Not used for npm install
        is_archive: false,
        archive_binary_path: None,
        install_method: Some(InstallMethod::Npm {
            package: "bash-language-server".to_string(),
            binary_name: "bash-language-server".to_string(),
        }),
    }
}

/// Dockerfile Language Server (npm).
pub fn dockerfile_language_server() -> DownloadableServer {
    DownloadableServer {
        id: "dockerfile-language-server".to_string(),
        name: "Dockerfile Language Server".to_string(),
        github_repo: "rcjsuen/dockerfile-language-server-nodejs".to_string(),
        binary_pattern: "docker-langserver{ext}".to_string(),
        asset_pattern: String::new(), // Not used for npm install
        is_archive: false,
        archive_binary_path: None,
        install_method: Some(InstallMethod::Npm {
            package: "dockerfile-language-server-nodejs".to_string(),
            binary_name: "docker-langserver".to_string(),
        }),
    }
}

/// TexLab (LaTeX Language Server).
pub fn texlab() -> DownloadableServer {
    let os_arch = if cfg!(target_os = "windows") {
        "x86_64-windows"
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "aarch64-macos"
        } else {
            "x86_64-macos"
        }
    } else {
        if cfg!(target_arch = "aarch64") {
            "aarch64-linux"
        } else {
            "x86_64-linux"
        }
    };

    DownloadableServer {
        id: "texlab".to_string(),
        name: "TexLab (LaTeX Language Server)".to_string(),
        github_repo: "latex-lsp/texlab".to_string(),
        binary_pattern: "texlab{ext}".to_string(),
        asset_pattern: format!("texlab-{}.tar.gz", os_arch),
        is_archive: true,
        archive_binary_path: Some("texlab".to_string()),
        install_method: None,
    }
}

/// Tinymist (Typst Language Server).
pub fn tinymist() -> DownloadableServer {
    let os_arch = if cfg!(target_os = "windows") {
        "x86_64-pc-windows-msvc"
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "aarch64-apple-darwin"
        } else {
            "x86_64-apple-darwin"
        }
    } else {
        if cfg!(target_arch = "aarch64") {
            "aarch64-unknown-linux-gnu"
        } else {
            "x86_64-unknown-linux-gnu"
        }
    };

    DownloadableServer {
        id: "tinymist".to_string(),
        name: "Tinymist (Typst Language Server)".to_string(),
        github_repo: "Myriad-Dreamin/tinymist".to_string(),
        binary_pattern: "tinymist{ext}".to_string(),
        asset_pattern: format!(
            "tinymist-{}{}",
            os_arch,
            if cfg!(target_os = "windows") {
                ".exe"
            } else {
                ""
            }
        ),
        is_archive: false,
        archive_binary_path: None,
        install_method: None,
    }
}

/// Clojure LSP.
pub fn clojure_lsp() -> DownloadableServer {
    let os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };

    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "amd64"
    };

    DownloadableServer {
        id: "clojure-lsp".to_string(),
        name: "Clojure LSP".to_string(),
        github_repo: "clojure-lsp/clojure-lsp".to_string(),
        binary_pattern: "clojure-lsp{ext}".to_string(),
        asset_pattern: format!("clojure-lsp-native-{}-{}.zip", os, arch),
        is_archive: true,
        archive_binary_path: Some("clojure-lsp".to_string()),
        install_method: None,
    }
}

/// Get all downloadable servers.
pub fn all() -> Vec<DownloadableServer> {
    vec![
        // Original servers
        gopls(),
        rust_analyzer(),
        pyright(),
        typescript_language_server(),
        vue_language_server(),
        svelte_language_server(),
        biome(),
        // New servers
        zls(),
        clangd(),
        lua_language_server(),
        elixir_ls(),
        jdtls(),
        terraform_ls(),
        yaml_language_server(),
        bash_language_server(),
        dockerfile_language_server(),
        texlab(),
        tinymist(),
        clojure_lsp(),
    ]
}

/// Find a downloadable server by ID.
pub fn find_by_id(id: &str) -> Option<DownloadableServer> {
    all().into_iter().find(|s| s.id == id)
}
