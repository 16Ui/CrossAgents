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
                    // Two entries can run one CLI; say which site one is fixed to, so the
                    // row is not just a name that looks like its neighbour.
                    let plugin = match &p.site {
                        Some(site) => {
                            // `internal`/`external` are the wire values; say what they mean.
                            let label = match site.as_str() {
                                "internal" => "国内站",
                                "external" => "国际站",
                                other => other,
                            };
                            format!("{} [{}]", p.name, label)
                        }
                        None if p.provider != p.name => format!("{} [{}]", p.name, p.provider),
                        None => p.name.clone(),
                    };
                    vec![plugin, models, efforts]
                })
                .collect();
            print_table(&["PLUGIN", "MODELS", "EFFORTS"], &rows);
        }
    }
    Ok(())
}
