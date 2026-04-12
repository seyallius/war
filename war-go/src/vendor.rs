//! vendor - Parse vendor/modules.txt and extract module metadata.
//!
//! Provides a pure function to parse Go's vendor manifest into
//! a structured Vec<ModuleInfo> for cache reconstruction.

use std::path::PathBuf;
use std::{fs, io, path::Path};
use war_core::{types::VendorModule, WarError};

// -------------------------------------------- Public API --------------------------------------------

/// Parses vendor/modules.txt and returns a list of vendored modules.
///
/// The format is:
///
///```text
/// ## <module_path> <version>
/// ### explicit; go <version>
/// <package_path>
/// <package_path>
/// ```
pub fn parse_vendor_manifest(project_root: &Path) -> Result<Vec<VendorModule>, WarError> {
    let manifest_path = project_root.join("vendor").join("modules.txt");
    if !manifest_path.exists() {
        return Err(WarError::IOError(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "vendor/modules.txt not found at {}",
                manifest_path.display()
            ),
        )));
    }

    let content = fs::read_to_string(&manifest_path).map_err(WarError::IOError)?;
    parse_modules_txt(&content)
}

/// Parses the raw content of a modules.txt string.
/// Separated from file I/O for easy unit testing.
/// modules.txt format is:
///
///```text
/// ## <module_path> <version>
/// ### explicit; go <version>
/// <package_path>
/// <package_path>
/// ```
pub fn parse_modules_txt(content: &str) -> Result<Vec<VendorModule>, WarError> {
    let mut modules: Vec<VendorModule> = Vec::new();
    let mut current: Option<VendorModule> = None;

    for (line_num, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        parse_modules_line(&mut modules, &mut current, line_num, &line)?;
    }

    // Don't forget the last module
    if let Some(module) = current.take() {
        modules.push(module);
    }

    Ok(modules)
}

// --------------------------------------------- Internal Helpers ---------------------------------------------

/// Parse single line entry in `vendors/modules.txt`.
///
/// Three cases:
/// 1. Module header e.g., "# <path> <version>"
/// 2. Metadata line e.g., "## explicit; go 1.20"
/// 3. Package line e.g., bare import path
fn parse_modules_line(
    modules: &mut Vec<VendorModule>,
    current: &mut Option<VendorModule>,
    line_num: usize,
    line: &&str,
) -> Result<(), WarError> {
    // Module header: "# <path> <version>"
    if line.starts_with("# ") && !line.starts_with("##") {
        let vendor_header = parse_header(modules, current, line_num, line)?;
        *current = Some(vendor_header)
    }
    // Metadata line: "## explicit; go 1.20"
    else if line.starts_with("## ") {
        parse_metadata(current, line_num, line)?
    }
    // Package line: bare import path
    else {
        parse_package(current, line_num, line)?
    }
    Ok(())
}

/// Parse header line i.e., first line entry:
/// ```text
/// ## <path> <version>
/// ```
fn parse_header(
    modules: &mut Vec<VendorModule>,
    current: &mut Option<VendorModule>,
    line_num: usize,
    line: &str,
) -> Result<VendorModule, WarError> {
    // Push the previous module if any
    if let Some(module) = current.take() {
        modules.push(module);
    }

    let parts: Vec<&str> = line[2..].splitn(2, ' ').collect();
    if parts.len() < 2 {
        return Err(WarError::ParseError(format!(
            "invalid module header at line {}: '{}'",
            line_num + 1,
            line
        )));
    }

    Ok(VendorModule {
        path: parts[0].to_string(),
        version: parts[1].to_string(),
        explicit: false,
        go_version: None,
        packages: Vec::new(),
        vendor_path: PathBuf::new(),
    })
}

/// Parse metadata line i.e., second line:
/// ```text
/// ### explicit; go <version>
/// ```
fn parse_metadata(
    current: &mut Option<VendorModule>,
    line_num: usize,
    line: &&str,
) -> Result<(), WarError> {
    let Some(ref mut module) = current else {
        return Err(WarError::ParseError(format!(
            "metadata line without module header at line {}: '{}'",
            line_num + 1,
            line
        )));
    };

    let meta = &line[3..];
    if meta.contains("explicit") {
        module.explicit = true;
    }
    // Extract go version: "explicit; go 1.20"
    if let Some(go_pos) = meta.find("go ") {
        let go_ver = meta[go_pos + 3..].trim().to_string();
        if !go_ver.is_empty() {
            module.go_version = Some(go_ver);
        }
    }
    Ok(())
}

/// Parse package line i.e., third line:
/// ```text
/// <bare_import_path>
/// ```
fn parse_package(
    current: &mut Option<VendorModule>,
    line_num: usize,
    line: &str,
) -> Result<(), WarError> {
    let Some(ref mut module) = current else {
        return Err(WarError::ParseError(format!(
            "package line without module header at line {}: '{}'",
            line_num + 1,
            line
        )));
    };
    module.packages.push(line.to_string());
    Ok(())
}

#[cfg(test)]
mod vendor_tests {
    use super::*;

    const SAMPLE_MODULES_TXT: &str = "\
# github.com/gin-gonic/gin v1.9.1
## explicit; go 1.20
github.com/gin-gonic/gin
github.com/gin-gonic/gin/binding
github.com/gin-gonic/gin/render
# golang.org/x/net v0.10.0
## explicit; go 1.17
golang.org/x/net/html
golang.org/x/net/html/atom
# github.com/stretchr/testify v1.8.4
## explicit; go 1.20
github.com/stretchr/testify/assert
";

    #[test]
    fn test_parse_modules_txt_basic() {
        let modules = parse_modules_txt(SAMPLE_MODULES_TXT).unwrap();

        assert_eq!(modules.len(), 3);

        assert_eq!(modules[0].path, "github.com/gin-gonic/gin");
        assert_eq!(modules[0].version, "v1.9.1");
        assert!(modules[0].explicit);
        assert_eq!(modules[0].go_version.as_deref(), Some("1.20"));
        assert_eq!(modules[0].packages.len(), 3);
        assert_eq!(modules[0].packages[0], "github.com/gin-gonic/gin");

        assert_eq!(modules[1].path, "golang.org/x/net");
        assert_eq!(modules[1].version, "v0.10.0");
        assert_eq!(modules[1].go_version.as_deref(), Some("1.17"));
        assert_eq!(modules[1].packages.len(), 2);

        assert_eq!(modules[2].path, "github.com/stretchr/testify");
        assert_eq!(modules[2].packages[0], "github.com/stretchr/testify/assert");
    }

    #[test]
    fn test_parse_empty_content() {
        let modules = parse_modules_txt("").unwrap();
        assert!(modules.is_empty());
    }

    #[test]
    fn test_parse_blank_lines_ignored() {
        let content = "\n\n# github.com/foo/bar v0.1.0\n## explicit\n\ngithub.com/foo/bar\n\n";
        let modules = parse_modules_txt(content).unwrap();

        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].path, "github.com/foo/bar");
        assert!(modules[0].explicit);
        assert!(modules[0].go_version.is_none());
    }

    #[test]
    fn test_parse_invalid_header_returns_error() {
        let content = "# incomplete-header\n";
        let result = parse_modules_txt(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_orphan_metadata_returns_error() {
        let content = "## explicit; go 1.20\n";
        let result = parse_modules_txt(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_orphan_package_returns_error() {
        let content = "github.com/some/package\n";
        let result = parse_modules_txt(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_no_explicit_flag() {
        let content = "# github.com/foo/bar v1.0.0\ngithub.com/foo/bar\n";
        let modules = parse_modules_txt(content).unwrap();

        assert_eq!(modules.len(), 1);
        assert!(!modules[0].explicit);
        assert!(modules[0].go_version.is_none());
    }
}
