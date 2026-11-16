//! The built-in agents, as descriptions: `agents/*.toml`, compiled into the hub
//! (docs/DESIGN.md §9.3). An agent added by hand uses the same format.

use std::sync::LazyLock;

use crossagents_drivers::locate::Cli;

use crate::{PluginConfig, manifest};

/// The descriptions, in the order `GET /v1/plugins` lists them, with `extends` resolved.
///
/// A description may start from another (`extends = "codebuddy"`) and change only what
/// differs: two entries over one CLI say that instead of copying the models, the locate
/// rules and the install commands. Bases come first here, so one pass resolves them.
static BUILTIN: LazyLock<Vec<(String, PluginConfig)>> = LazyLock::new(|| {
    let sources = [
        ("claude", include_str!("../agents/claude.toml")),
        ("codex", include_str!("../agents/codex.toml")),
        ("codebuddy", include_str!("../agents/codebuddy.toml")),
        ("workbuddy-cn", include_str!("../agents/workbuddy-cn.toml")),
        (
            "workbuddy-intl",
            include_str!("../agents/workbuddy-intl.toml"),
        ),
        ("with", include_str!("../agents/with.toml")),
        ("cursor", include_str!("../agents/cursor.toml")),
        ("gemini", include_str!("../agents/gemini.toml")),
        ("antigravity", include_str!("../agents/antigravity.toml")),
        ("tokenworks", include_str!("../agents/tokenworks.toml")),
        (
            "deepseek-harness",
            include_str!("../agents/deepseek-harness.toml"),
        ),
    ];
    // Two passes, not a map: a description may extend another (`extends = "codebuddy"`), so
    // each entry is resolved against the bases declared before it.
    let mut out: Vec<(String, PluginConfig)> = Vec::with_capacity(sources.len());
    for (name, text) in sources {
        let (name, config) = manifest::parse(text, name)
            .unwrap_or_else(|err| panic!("built-in agent description {name}.toml: {err}"));
        let config = match config.extends.clone() {
            None => config,
            Some(base) => {
                let Some((_, base)) = out.iter().find(|(declared, _)| *declared == base) else {
                    panic!(
                        "built-in agent description {name}.toml extends {base:?}, which is \
                         declared after it (a base must come first)"
                    );
                };
                base.clone().overlay(&config)
            }
        };
        out.push((name, config));
    }
    out
});

/// Every built-in description, in the order the API lists them.
pub fn all() -> &'static [(String, PluginConfig)] {
    &BUILTIN
}

pub fn config(name: &str) -> Option<&'static PluginConfig> {
    BUILTIN
        .iter()
        .find(|(built_in, _)| built_in == name)
        .map(|(_, config)| config)
}

/// The CLI a built-in description names, for callers outside the registry (account probes).
pub fn cli(name: &str) -> Option<Cli> {
    config(name).map(PluginConfig::cli)
}

/// Where a built-in description looks for its CLI, for callers that find the program
/// themselves (account probes).
pub fn locator(name: &str) -> Option<crossagents_drivers::locate::Locator> {
    config(name).map(|config| config.locate.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_descriptions_load() {
        let names: Vec<&str> = all().iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "claude",
                "codex",
                "codebuddy",
                "workbuddy-cn",
                "workbuddy-intl",
                "with",
                "cursor",
                "gemini",
                "antigravity",
                "tokenworks",
                "deepseek-harness"
            ]
        );
        for (name, config) in all() {
            assert!(config.driver.is_some(), "{name} needs a driver");
            assert!(
                config.install.hint.is_some() || !config.install.methods.is_empty(),
                "{name} should say how to install it"
            );
            assert!(
                !config.locate.is_empty(),
                "{name} should say where its CLI is"
            );
        }
    }

    #[test]
    fn an_extension_inherits_its_base() {
        let base = config("codebuddy").expect("base");
        let cn = config("workbuddy-cn").expect("derived");
        let intl = config("workbuddy-intl").expect("derived");
        // Everything but the site, the label and the name comes from the base.
        assert_eq!(cn.command, base.command);
        assert_eq!(cn.models, base.models);
        assert_eq!(cn.efforts, base.efforts);
        assert_eq!(cn.native_command, base.native_command);
        assert_eq!(cn.locate, base.locate);
        // And each entry is its own site.
        assert_ne!(cn.env, intl.env);
        assert_eq!(cn.provider.as_deref(), Some("codebuddy"));
        assert!(base.hidden, "the base is not an entry of its own");
    }
}
