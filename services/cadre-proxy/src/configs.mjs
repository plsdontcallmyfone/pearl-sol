// One-command client configs pointing at the proxy. Sources (checked 2026-10-19):
// - Hermes Agent: NousResearch/hermes-agent cli-config.yaml.example (model.provider "custom",
//   model.base_url, model.default, model.api_key). The `hermes config set` form is from
//   NVIDIA's Hermes playbook (build.nvidia.com/playbooks/hermes-agent), not Nous' own docs.
// - OpenClaw: docs.openclaw.ai/concepts/model-providers/custom-providers (models.providers.<id>
//   {baseUrl, apiKey, api: "openai-completions", models: [{id, name, contextWindow, maxTokens}]},
//   agents.defaults.model.primary "<id>/<model>"; onboarding flags --custom-base-url,
//   --custom-model-id, --custom-no-auth are named there, the combined command is not shown).
// - Kilo Code: kilo.ai/docs/code-with-ai/agents/custom-models (kilo.json provider
//   "openai-compatible" with options.apiKey / options.baseURL and models.<id>.limit; global
//   config under ~/.config/kilo; `kilo models` lists models).
// - Cline: docs.cline.bot/provider-config/openai-compatible (settings panel fields only:
//   API Provider "OpenAI Compatible", Base URL, API Key, Model ID). No file or CLI form is
//   documented there, so none is printed.

export const CLIENTS = ["hermes", "openclaw", "kilo", "cline"];

export function configSnippet(client, { baseUrl, model, apiKey, contextLength = 4096, maxTokens = 1024 }) {
  const v1 = baseUrl.replace(/\/+$/, "") + "/v1";
  const ctxNote = `# Cadre's served context is ${contextLength} tokens (GET /v1/models context_length); agents that need long contexts will not fit.`;
  switch (client) {
    case "hermes":
      return [
        "# Hermes Agent (Nous Research): point the main model at cadre-proxy.",
        "# Keys from NousResearch/hermes-agent cli-config.yaml.example; `hermes config set` form per NVIDIA's Hermes playbook.",
        ctxNote,
        `hermes config set model.provider custom && hermes config set model.base_url ${v1} && hermes config set model.default ${model} && hermes config set model.api_key ${apiKey}`,
        "",
        "# or in ~/.hermes/config.yaml:",
        "# model:",
        `#   default: "${model}"`,
        `#   provider: "custom"`,
        `#   base_url: "${v1}"`,
        `#   api_key: "${apiKey}"`,
      ].join("\n");
    case "openclaw": {
      const cfg = {
        models: {
          providers: {
            cadre: { baseUrl: v1, apiKey, api: "openai-completions", models: [{ id: model, name: `Cadre ${model}`, contextWindow: contextLength, maxTokens }] },
          },
        },
        agents: { defaults: { model: { primary: `cadre/${model}` } } },
      };
      return [
        "# OpenClaw: add cadre-proxy as a custom provider (docs.openclaw.ai/concepts/model-providers/custom-providers).",
        ctxNote,
        "# One command (onboarding flags named in the OpenClaw docs; the proxy needs no key unless started with --token):",
        `openclaw onboard --custom-base-url ${v1} --custom-model-id ${model} --custom-no-auth`,
        "",
        "# or merge into ~/.openclaw/openclaw.json:",
        JSON.stringify(cfg, null, 2),
      ].join("\n");
    }
    case "kilo": {
      const cfg = {
        $schema: "https://app.kilo.ai/config.json",
        model: `openai-compatible/${model}`,
        provider: { "openai-compatible": { options: { apiKey, baseURL: v1 }, models: { [model]: { name: `Cadre ${model}`, tool_call: true, limit: { context: contextLength, output: maxTokens } } } } },
      };
      return [
        "# Kilo Code: custom OpenAI-compatible provider (kilo.ai/docs/code-with-ai/agents/custom-models).",
        ctxNote,
        "# One command (writes the global config only when none exists; otherwise merge the provider block by hand), then `kilo models`:",
        `mkdir -p ~/.config/kilo && test ! -e ~/.config/kilo/kilo.json && cat > ~/.config/kilo/kilo.json <<'JSON'\n${JSON.stringify(cfg, null, 2)}\nJSON`,
      ].join("\n");
    }
    case "cline":
      return [
        "# Cline: Settings (gear icon) > API Provider (docs.cline.bot/provider-config/openai-compatible).",
        "# The Cline docs show these settings-panel fields only; no config file or CLI form is documented, so none is given here.",
        ctxNote,
        `#   API Provider: OpenAI Compatible`,
        `#   Base URL:     ${v1}`,
        `#   API Key:      ${apiKey}`,
        `#   Model ID:     ${model}`,
        `#   Model Configuration > Context Window: ${contextLength}`,
      ].join("\n");
    default:
      throw new Error(`unknown client ${client}; one of ${CLIENTS.join(", ")}`);
  }
}
