#!/usr/bin/env node
import { spawn } from "child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "fs";
import path from "path";
import { fileURLToPath } from "url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const rootDir = path.resolve(__dirname, "..");
const ragDir = path.join(rootDir, "tools", "ai-workspace", "rag-mcp-server");
const proxyDir = path.join(rootDir, "tools", "ai-workspace", "openrouter-proxy");
const nativeBinary = process.env.CODE_BINARY_PATH;
const nativeWrapper = path.join(rootDir, "codex-cli", "bin", "coder.js");
const npmCmd = process.platform === "win32" ? "npm.cmd" : "npm";
const aiSettingsPath = path.join(rootDir, "tools", "ai-workspace", "ai-settings.json");
const aiHomeDefault = process.platform === "win32"
  ? path.join(process.env.USERPROFILE ?? rootDir, ".every-code-ai")
  : path.join(process.env.HOME ?? rootDir, ".every-code-ai");

const ragGuiUrl = (process.env.RAG_GUI_URL ?? "http://127.0.0.1:8787").replace(/\/+$/, "");
const proxyBaseUrl = (process.env.PROXY_BASE_URL ?? `http://127.0.0.1:${process.env.PROXY_PORT ?? process.env.PORT ?? "8080"}`).replace(/\/+$/, "");

function loadAiSettings() {
  const fallback = {
    proxyBaseUrl,
    proxyPort: Number(process.env.PROXY_PORT ?? process.env.PORT ?? 8080),
    uiPort: Number(process.env.UI_PORT ?? 3000),
    ragGuiUrl,
    codeHome: aiHomeDefault,
    openrouterProxyBaseUrl: `${proxyBaseUrl.replace(/\/+$/, "")}/v1`,
    openrouterApiKey: process.env.OPENROUTER_API_KEY ?? "",
  };

  try {
    if (!existsSync(aiSettingsPath)) {
      return fallback;
    }
    const parsed = JSON.parse(readFileSync(aiSettingsPath, "utf8"));
    return { ...fallback, ...parsed };
  } catch (err) {
    console.error(`[ai:dev] Failed to read AI settings at ${aiSettingsPath}: ${err?.message || err}`);
    process.exit(1);
  }
}

function ensureAiHomeConfig(aiHome, settings) {
  mkdirSync(aiHome, { recursive: true });
  const configPath = path.join(aiHome, "config.toml");
  const providerBaseUrl = String(
    settings.openrouterProxyBaseUrl ?? `${proxyBaseUrl.replace(/\/+$/, "")}/v1`
  ).replace(/\/+$/, "");
  const toml = `model_provider = "local-proxy"

[model_providers.local-proxy]
name = "Local Proxy"
base_url = "${providerBaseUrl}"
env_key = "OPENROUTER_API_KEY"
`;
  writeFileSync(configPath, toml, "utf8");
  return configPath;
}

function spawnManaged(cmd, args, cwd, label) {
  const child = process.platform === "win32"
    ? spawn("cmd.exe", ["/d", "/s", "/c", [cmd, ...args].join(" ")], {
        cwd,
        env: { ...process.env },
        stdio: "inherit",
        shell: false,
      })
    : spawn(cmd, args, {
        cwd,
        env: { ...process.env },
        stdio: "inherit",
        shell: false,
      });
  child.on("exit", (code, signal) => {
    if (signal) {
      console.error(`[ai:dev] ${label} child exited via ${signal}`);
    } else {
      console.error(`[ai:dev] ${label} child exited (${code ?? 1})`);
    }
  });
  return child;
}

function resolveCliLaunchCommand() {
  if (nativeBinary && existsSync(nativeBinary)) {
    return {
      cmd: nativeBinary,
      args: process.argv.slice(2),
      label: "native Every Code binary"
    };
  }

  if (existsSync(nativeWrapper)) {
    return {
      cmd: process.execPath,
      args: [nativeWrapper, ...process.argv.slice(2)],
      label: "in-repo coder.js wrapper"
    };
  }

  console.error("[ai:dev] Unable to find a native Every Code binary or the in-repo codex-cli wrapper.");
  process.exit(1);
}

async function isReachable(url) {
  try {
    const resp = await fetch(url, { method: "GET" });
    return resp.ok || resp.status >= 400;
  } catch {
    return false;
  }
}

async function waitFor(label, url, maxMs = 15000) {
  const started = Date.now();
  while (Date.now() - started < maxMs) {
    if (await isReachable(url)) return true;
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  console.error(`[ai:dev] ${label} did not become reachable at ${url}`);
  return false;
}

const background = [];
const aiSettings = loadAiSettings();
const aiHome = String(aiSettings.codeHome ?? aiHomeDefault).replace(/%USERPROFILE%/g, process.env.USERPROFILE ?? "");
const aiConfigPath = ensureAiHomeConfig(aiHome, aiSettings);
const openrouterApiKey = String(aiSettings.openrouterApiKey ?? process.env.OPENROUTER_API_KEY ?? "").trim();
const proxyBaseUrlFromSettings = String(aiSettings.openrouterProxyBaseUrl ?? `${proxyBaseUrl.replace(/\/+$/, "")}/v1`).trim();
const cliLaunch = resolveCliLaunchCommand();

const ragReady = await isReachable(ragGuiUrl);
if (!ragReady) {
  background.push(spawnManaged(npmCmd, ["run", "gui:backend"], ragDir, "rag-mcp-server"));
}

const proxyReady = await isReachable(proxyBaseUrl);
if (!proxyReady) {
  background.push(spawnManaged(npmCmd, ["start"], proxyDir, "openrouter-proxy"));
}

if (!ragReady) {
  const ok = await waitFor("RAG GUI backend", ragGuiUrl);
  if (!ok) {
    for (const child of background) {
      try { child.kill(); } catch {}
    }
    process.exit(1);
  }
} else {
  console.log(`[ai:dev] RAG GUI already reachable at ${ragGuiUrl}; not starting a duplicate.`);
}

if (!proxyReady) {
  const ok = await waitFor("OpenRouter proxy", proxyBaseUrl);
  if (!ok) {
    for (const child of background) {
      try { child.kill(); } catch {}
    }
    process.exit(1);
  }
} else {
  console.log(`[ai:dev] Proxy already reachable at ${proxyBaseUrl}; not starting a duplicate.`);
}

console.log(`[ai:dev] AI services are ready. Launching Every Code CLI via ${cliLaunch.label}...`);
const cli = spawn(cliLaunch.cmd, cliLaunch.args, {
  cwd: rootDir,
  env: {
    ...process.env,
    CODEX_HOME: aiHome,
    CODE_HOME: aiHome,
    EVERY_CODE_HOME: aiHome,
    AI_WORKSPACE_HOME: aiHome,
    AI_WORKSPACE_CONFIG: aiConfigPath,
    EVERY_CODE_PROXY_BASE_URL: proxyBaseUrlFromSettings,
    OPENROUTER_API_KEY: openrouterApiKey,
  },
  stdio: "inherit",
  shell: false,
});

const cleanup = () => {
  for (const child of background) {
    try {
      child.kill();
    } catch {}
  }
};

for (const sig of ["SIGINT", "SIGTERM", "SIGHUP"]) {
  process.on(sig, () => {
    try { cli.kill(sig); } catch {}
    cleanup();
  });
}

cli.on("exit", (code, signal) => {
  cleanup();
  if (signal) {
    process.kill(process.pid, signal);
  } else {
    process.exit(code ?? 0);
  }
});
