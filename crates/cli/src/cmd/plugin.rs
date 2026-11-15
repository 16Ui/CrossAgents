use anyhow::Result;
use clap::Subcommand;
use crossagents_api_types::ListPluginsResponse;

use super::Context;
use crate::output::{print_json, print_table};

#[derive(Debug, Subcommand)]
pub enum PluginCommand {
    /// List plugins and the models they know.
    Ls,
}

pub async fn run(ctx: &Context, cmd: PluginCommand) -> Result<()> {
    match cmd {
        PluginCommand::Ls => {
            let list: ListPluginsResponse = ctx.client()?.get("/v1/plugins").await?;
            if ctx.json {
                return print_json(&list);
            }
            let rows: Vec<Vec<String>> = list
                .plugins
                .iter()
                .map(|p| {
                    let models = if p.models.is_empty() {
                        "(agent default)".to_owned()
                    } else {
                        p.models.join(", ")
                    };
                    let efforts = if p.efforts.is_empty() {
                        "-".to_owned()
                    } else {
                        p.efforts.join(", ")
                    };
                    // What the description calls itself, in its own script; the name stays a
                    // slug, shown beside a label that differs from it. Two entries can run one
                    // CLI, so a fixed site is said too.
                    let mut plugin = if p.label == p.name {
                        p.name.clone()
                    } else {
                        format!("{} ({})", p.label, p.name)
                    };
                    if let Some(site) = p.site.as_deref() {
                        // `internal`/`external` are the wire values; say what they mean.
                        let site = match site {
                            "internal" => "国内站",
                            "external" => "国际站",
                            other => other,
                        };
                        plugin.push_str(&format!(" [{site}]"));
                    }
                    vec![plugin, models, efforts]
                })
                .collect();
            print_table(&["PLUGIN", "MODELS", "EFFORTS"], &rows);
        }
    }
    Ok(())
}
