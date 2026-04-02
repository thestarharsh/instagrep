//! Built-in file type definitions, matching ripgrep's type system.

use std::collections::HashMap;

/// Returns the built-in file type map: type_name → list of glob patterns.
pub fn builtin_type_map() -> HashMap<&'static str, &'static [&'static str]> {
    let mut m = HashMap::new();
    m.insert("agda", &["*.agda", "*.lagda"][..]);
    m.insert("aidl", &["*.aidl"][..]);
    m.insert("amake", &["*.mk", "*.bp"][..]);
    m.insert("asciidoc", &["*.adoc", "*.asc", "*.asciidoc"][..]);
    m.insert("asm", &["*.asm", "*.s", "*.S"][..]);
    m.insert("avro", &["*.avdl", "*.avpr", "*.avsc"][..]);
    m.insert("awk", &["*.awk"][..]);
    m.insert("bazel", &["*.bzl", "BUILD", "BUILD.bazel", "WORKSPACE", "WORKSPACE.bazel"][..]);
    m.insert("bitbake", &["*.bb", "*.bbappend", "*.bbclass", "*.conf", "*.inc"][..]);
    m.insert("c", &["*.c", "*.h", "*.H"][..]);
    m.insert("cabal", &["*.cabal"][..]);
    m.insert("cbor", &["*.cbor"][..]);
    m.insert("ceylon", &["*.ceylon"][..]);
    m.insert("clojure", &["*.clj", "*.cljc", "*.cljs", "*.cljx"][..]);
    m.insert("cmake", &["*.cmake", "CMakeLists.txt"][..]);
    m.insert("coffeescript", &["*.coffee"][..]);
    m.insert("config", &["*.cfg", "*.conf", "*.config", "*.ini"][..]);
    m.insert("cpp", &["*.cpp", "*.cc", "*.cxx", "*.c++", "*.hpp", "*.hh", "*.hxx", "*.h++", "*.h", "*.inl"][..]);
    m.insert("crystal", &["*.cr"][..]);
    m.insert("cs", &["*.cs"][..]);
    m.insert("csharp", &["*.cs"][..]);
    m.insert("css", &["*.css", "*.scss", "*.sass", "*.less"][..]);
    m.insert("csv", &["*.csv"][..]);
    m.insert("cuda", &["*.cu", "*.cuh"][..]);
    m.insert("cython", &["*.pyx", "*.pxd", "*.pxi"][..]);
    m.insert("d", &["*.d", "*.di"][..]);
    m.insert("dart", &["*.dart"][..]);
    m.insert("dhall", &["*.dhall"][..]);
    m.insert("docker", &["Dockerfile", "*.dockerfile"][..]);
    m.insert("elixir", &["*.ex", "*.exs"][..]);
    m.insert("elm", &["*.elm"][..]);
    m.insert("erb", &["*.erb"][..]);
    m.insert("erlang", &["*.erl", "*.hrl"][..]);
    m.insert("fish", &["*.fish"][..]);
    m.insert("fortran", &["*.f", "*.F", "*.f77", "*.f90", "*.F90", "*.f95", "*.f03", "*.for", "*.fpp"][..]);
    m.insert("fsharp", &["*.fs", "*.fsi", "*.fsx"][..]);
    m.insert("gn", &["*.gn", "*.gni"][..]);
    m.insert("go", &["*.go"][..]);
    m.insert("gradle", &["*.gradle"][..]);
    m.insert("graphql", &["*.graphql", "*.graphqls", "*.gql"][..]);
    m.insert("groovy", &["*.groovy", "*.gradle"][..]);
    m.insert("h", &["*.h", "*.hpp", "*.hh", "*.hxx"][..]);
    m.insert("haml", &["*.haml"][..]);
    m.insert("haskell", &["*.hs", "*.lhs", "*.cabal"][..]);
    m.insert("hbs", &["*.hbs"][..]);
    m.insert("hs", &["*.hs", "*.lhs"][..]);
    m.insert("html", &["*.html", "*.htm", "*.xhtml"][..]);
    m.insert("idris", &["*.idr", "*.lidr"][..]);
    m.insert("java", &["*.java"][..]);
    m.insert("jinja", &["*.j2", "*.jinja", "*.jinja2"][..]);
    m.insert("js", &["*.js", "*.mjs", "*.cjs", "*.jsx"][..]);
    m.insert("json", &["*.json", "*.jsonl", "*.geojson"][..]);
    m.insert("jsonl", &["*.jsonl"][..]);
    m.insert("julia", &["*.jl"][..]);
    m.insert("jupyter", &["*.ipynb"][..]);
    m.insert("kotlin", &["*.kt", "*.kts"][..]);
    m.insert("less", &["*.less"][..]);
    m.insert("license", &["LICENSE", "LICENSE.*", "LICENCE", "LICENCE.*", "COPYING", "COPYING.*"][..]);
    m.insert("lisp", &["*.el", "*.lisp", "*.lsp", "*.cl", "*.fasl"][..]);
    m.insert("lock", &["*.lock", "package-lock.json", "yarn.lock", "Cargo.lock", "Gemfile.lock"][..]);
    m.insert("log", &["*.log"][..]);
    m.insert("lua", &["*.lua"][..]);
    m.insert("m4", &["*.m4"][..]);
    m.insert("make", &["Makefile", "makefile", "GNUmakefile", "*.mk", "*.mak"][..]);
    m.insert("mako", &["*.mako", "*.mao"][..]);
    m.insert("markdown", &["*.md", "*.markdown", "*.mdown", "*.mkdn"][..]);
    m.insert("md", &["*.md", "*.markdown"][..]);
    m.insert("nim", &["*.nim", "*.nimble"][..]);
    m.insert("nix", &["*.nix"][..]);
    m.insert("objc", &["*.m", "*.mm"][..]);
    m.insert("objcpp", &["*.mm"][..]);
    m.insert("ocaml", &["*.ml", "*.mli", "*.mll", "*.mly"][..]);
    m.insert("org", &["*.org"][..]);
    m.insert("pascal", &["*.pas", "*.dpr", "*.lpr", "*.pp", "*.inc"][..]);
    m.insert("perl", &["*.pl", "*.pm", "*.t", "*.psgi"][..]);
    m.insert("php", &["*.php", "*.phtml", "*.php3", "*.php4", "*.php5", "*.php7", "*.phps"][..]);
    m.insert("pod", &["*.pod"][..]);
    m.insert("protobuf", &["*.proto"][..]);
    m.insert("ps", &["*.ps1", "*.psm1", "*.psd1"][..]);
    m.insert("puppet", &["*.pp", "*.erb"][..]);
    m.insert("py", &["*.py", "*.pyi"][..]);
    m.insert("python", &["*.py", "*.pyi"][..]);
    m.insert("qmake", &["*.pro", "*.pri"][..]);
    m.insert("r", &["*.r", "*.R", "*.Rmd", "*.Rnw"][..]);
    m.insert("rdoc", &["*.rdoc"][..]);
    m.insert("readme", &["README", "README.*"][..]);
    m.insert("robot", &["*.robot"][..]);
    m.insert("rst", &["*.rst"][..]);
    m.insert("ruby", &["*.rb", "*.gemspec", "Gemfile", "Rakefile"][..]);
    m.insert("rust", &["*.rs"][..]);
    m.insert("sass", &["*.sass", "*.scss"][..]);
    m.insert("scala", &["*.scala", "*.sbt"][..]);
    m.insert("sh", &["*.sh", "*.bash", "*.zsh", "*.fish", ".bashrc", ".zshrc", ".bash_profile"][..]);
    m.insert("slim", &["*.slim"][..]);
    m.insert("smarty", &["*.tpl"][..]);
    m.insert("sml", &["*.sml", "*.sig", "*.fun"][..]);
    m.insert("solidity", &["*.sol"][..]);
    m.insert("sql", &["*.sql"][..]);
    m.insert("stylus", &["*.styl"][..]);
    m.insert("sv", &["*.v", "*.sv", "*.svh", "*.svi"][..]);
    m.insert("svg", &["*.svg"][..]);
    m.insert("swift", &["*.swift"][..]);
    m.insert("swig", &["*.i", "*.swg"][..]);
    m.insert("tcl", &["*.tcl"][..]);
    m.insert("terraform", &["*.tf", "*.tfvars"][..]);
    m.insert("tex", &["*.tex", "*.ltx", "*.cls", "*.sty", "*.bib"][..]);
    m.insert("textile", &["*.textile"][..]);
    m.insert("thrift", &["*.thrift"][..]);
    m.insert("toml", &["*.toml"][..]);
    m.insert("ts", &["*.ts", "*.tsx", "*.mts", "*.cts"][..]);
    m.insert("typescript", &["*.ts", "*.tsx", "*.mts", "*.cts"][..]);
    m.insert("txt", &["*.txt"][..]);
    m.insert("vala", &["*.vala"][..]);
    m.insert("vb", &["*.vb"][..]);
    m.insert("verilog", &["*.v", "*.sv", "*.svh"][..]);
    m.insert("vhdl", &["*.vhd", "*.vhdl"][..]);
    m.insert("vim", &["*.vim"][..]);
    m.insert("vue", &["*.vue"][..]);
    m.insert("webidl", &["*.idl", "*.webidl", "*.widl"][..]);
    m.insert("wiki", &["*.mediawiki", "*.wiki"][..]);
    m.insert("xml", &["*.xml", "*.xsl", "*.xslt", "*.xsd", "*.wsdl", "*.svg", "*.plist"][..]);
    m.insert("yaml", &["*.yaml", "*.yml"][..]);
    m.insert("zig", &["*.zig"][..]);
    m
}

/// Format the type list for display (matching `rg --type-list` output).
pub fn format_type_list() -> String {
    let map = builtin_type_map();
    let mut types: Vec<_> = map.into_iter().collect();
    types.sort_by_key(|(name, _)| *name);

    let mut out = String::new();
    for (name, globs) in types {
        out.push_str(name);
        out.push_str(": ");
        out.push_str(&globs.join(", "));
        out.push('\n');
    }
    out
}

/// Given a type name, return the glob patterns, or None if unknown.
pub fn type_globs(name: &str) -> Option<&'static [&'static str]> {
    builtin_type_map().get(name).copied()
}

/// Check if a file path matches any of the given type names.
pub fn matches_type(path: &str, type_names: &[String]) -> bool {
    let map = builtin_type_map();
    for type_name in type_names {
        if let Some(globs) = map.get(type_name.as_str()) {
            for glob in *globs {
                if glob_matches(glob, path) {
                    return true;
                }
            }
        }
    }
    false
}

/// Check if a file path matches any of the given type names (for exclusion).
pub fn matches_type_not(path: &str, type_names: &[String]) -> bool {
    matches_type(path, type_names)
}

/// Simple glob match for type patterns.
fn glob_matches(pattern: &str, path: &str) -> bool {
    if pattern.starts_with("*.") {
        path.ends_with(&pattern[1..])
    } else {
        // Exact filename match (e.g., "Makefile", "Dockerfile")
        path.ends_with(pattern)
            && (path.len() == pattern.len()
                || path.as_bytes()[path.len() - pattern.len() - 1] == b'/')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_map_has_common_types() {
        let map = builtin_type_map();
        assert!(map.contains_key("rust"));
        assert!(map.contains_key("py"));
        assert!(map.contains_key("js"));
        assert!(map.contains_key("go"));
        assert!(map.contains_key("java"));
        assert!(map.contains_key("ts"));
        assert!(map.contains_key("cpp"));
    }

    #[test]
    fn test_matches_type() {
        assert!(matches_type("src/main.rs", &["rust".to_string()]));
        assert!(!matches_type("src/main.rs", &["py".to_string()]));
        assert!(matches_type("test.py", &["py".to_string()]));
        assert!(matches_type("src/app.tsx", &["ts".to_string()]));
    }

    #[test]
    fn test_matches_type_exact_filename() {
        assert!(matches_type("Makefile", &["make".to_string()]));
        assert!(matches_type("src/Makefile", &["make".to_string()]));
        assert!(matches_type("Dockerfile", &["docker".to_string()]));
    }

    #[test]
    fn test_format_type_list() {
        let list = format_type_list();
        assert!(list.contains("rust: *.rs"));
        assert!(list.contains("py: *.py, *.pyi"));
    }

    #[test]
    fn test_glob_matches() {
        assert!(glob_matches("*.rs", "main.rs"));
        assert!(glob_matches("*.rs", "src/main.rs"));
        assert!(!glob_matches("*.rs", "main.py"));
        assert!(glob_matches("Makefile", "Makefile"));
        assert!(glob_matches("Makefile", "src/Makefile"));
        assert!(!glob_matches("Makefile", "Makefile.bak"));
    }
}
