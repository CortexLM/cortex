//! Tests for the LSP downloader module.

#[cfg(test)]
mod tests {
    use crate::downloader::{
        archive::validate_path_safe, core::LspDownloader, servers, types::InstallMethod,
    };

    #[test]
    fn test_resolve_pattern() {
        let temp_dir = std::env::temp_dir().join("lsp");
        let downloader = LspDownloader::with_base_dir(&temp_dir).unwrap();
        let result = downloader.resolve_pattern("binary-{os}-{arch}{ext}", "1.0.0");

        #[cfg(target_os = "linux")]
        assert!(result.contains("linux"));

        #[cfg(target_os = "macos")]
        assert!(result.contains("darwin"));

        #[cfg(target_os = "windows")]
        assert!(result.contains("windows") && result.ends_with(".exe"));
    }

    #[test]
    fn test_matches_pattern() {
        assert!(LspDownloader::matches_pattern(
            "gopls.linux-amd64",
            "gopls.*"
        ));
        assert!(LspDownloader::matches_pattern(
            "rust-analyzer-x86_64-unknown-linux-gnu.gz",
            "rust-analyzer-*.gz"
        ));
        assert!(!LspDownloader::matches_pattern("foo.zip", "bar-*.zip"));
    }

    #[test]
    fn test_path_validation_safe_paths() {
        let dest = std::env::temp_dir().join("test");

        // Normal paths should be accepted
        assert!(validate_path_safe(&dest, "file.txt").is_ok());
        assert!(validate_path_safe(&dest, "subdir/file.txt").is_ok());
        assert!(validate_path_safe(&dest, "a/b/c/file.txt").is_ok());
    }

    #[test]
    fn test_path_validation_traversal_attacks() {
        let dest = std::env::temp_dir().join("test");

        // Parent directory traversal should be rejected
        assert!(validate_path_safe(&dest, "../etc/passwd").is_err());
        assert!(validate_path_safe(&dest, "foo/../../../etc/passwd").is_err());
        assert!(validate_path_safe(&dest, "..").is_err());
        assert!(validate_path_safe(&dest, "foo/..").is_err());
    }

    #[test]
    fn test_path_validation_absolute_paths() {
        let dest = std::env::temp_dir().join("test");

        // Absolute paths should be rejected
        #[cfg(unix)]
        assert!(validate_path_safe(&dest, "/etc/passwd").is_err());

        #[cfg(windows)]
        assert!(validate_path_safe(&dest, "C:\\Windows\\System32").is_err());
    }

    #[test]
    fn test_path_validation_null_bytes() {
        let dest = std::env::temp_dir().join("test");

        // Null bytes should be rejected
        assert!(validate_path_safe(&dest, "file\0.txt").is_err());
    }

    #[test]
    fn test_all_servers() {
        let servers = servers::all();
        assert!(!servers.is_empty());
        // Original servers
        assert!(servers.iter().any(|s| s.id == "gopls"));
        assert!(servers.iter().any(|s| s.id == "rust-analyzer"));
        assert!(servers.iter().any(|s| s.id == "pyright"));
        assert!(servers.iter().any(|s| s.id == "typescript-language-server"));
        assert!(servers.iter().any(|s| s.id == "vue-language-server"));
        assert!(servers.iter().any(|s| s.id == "svelte-language-server"));
        assert!(servers.iter().any(|s| s.id == "biome"));
        // New servers
        assert!(servers.iter().any(|s| s.id == "zls"));
        assert!(servers.iter().any(|s| s.id == "clangd"));
        assert!(servers.iter().any(|s| s.id == "lua-language-server"));
        assert!(servers.iter().any(|s| s.id == "elixir-ls"));
        assert!(servers.iter().any(|s| s.id == "jdtls"));
        assert!(servers.iter().any(|s| s.id == "terraform-ls"));
        assert!(servers.iter().any(|s| s.id == "yaml-language-server"));
        assert!(servers.iter().any(|s| s.id == "bash-language-server"));
        assert!(servers.iter().any(|s| s.id == "dockerfile-language-server"));
        assert!(servers.iter().any(|s| s.id == "texlab"));
        assert!(servers.iter().any(|s| s.id == "tinymist"));
        assert!(servers.iter().any(|s| s.id == "clojure-lsp"));
    }

    #[test]
    fn test_npm_servers() {
        let yaml_ls = servers::yaml_language_server();
        assert!(matches!(
            yaml_ls.install_method,
            Some(InstallMethod::Npm { .. })
        ));

        let bash_ls = servers::bash_language_server();
        assert!(matches!(
            bash_ls.install_method,
            Some(InstallMethod::Npm { .. })
        ));

        let dockerfile_ls = servers::dockerfile_language_server();
        assert!(matches!(
            dockerfile_ls.install_method,
            Some(InstallMethod::Npm { .. })
        ));
    }

    #[test]
    fn test_find_by_id() {
        assert!(servers::find_by_id("zls").is_some());
        assert!(servers::find_by_id("clangd").is_some());
        assert!(servers::find_by_id("lua-language-server").is_some());
        assert!(servers::find_by_id("nonexistent").is_none());
    }
}
