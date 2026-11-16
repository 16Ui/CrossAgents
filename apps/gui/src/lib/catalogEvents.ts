/** Quota/model discovery can refresh in place; identity and installation changes expire it. */
export function catalogEvent(type: string): "reset" | "refresh" | undefined {
  if (["account.created", "account.removed", "account.login_finished", "plugin.changed"].includes(type)) return "reset";
  if (type.startsWith("account.") || type.startsWith("cli.install.")) return "refresh";
  return undefined;
}
