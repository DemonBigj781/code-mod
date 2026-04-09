#!/usr/bin/env node
import { existsSync, readFileSync } from "fs";
import path from "path";
import { fileURLToPath } from "url";
import { spawnSync } from "child_process";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const rootDir = path.resolve(__dirname, "..");
const ragDir = path.join(rootDir, "tools", "ai-workspace", "rag-mcp-server");
const proxyDir = path.join(rootDir, "tools", "ai-workspace", "openrouter-proxy");
const npmCmd = process.platform === "win32" ? "npm.cmd" : "npm";

const ragGuiUrl = (process.env.RAG_GUI_URL ?? "http://127.0.0.1:8787").replace(/\/+$/, "");
const proxyBaseUrl = (process.env.PROXY_BASE_URL ?? `http://127.0.0.1:${process.env.PROXY_PORT ?? process.env.PORT ?? "8080"}`).replace(/\/+$/, "");
const aiHomeDefault = process.platform === "win32"
  ? path.join(process.env.USERPROFILE ?? rootDir, ".every-code-ai")
  : path.join(process.env.HOME ?? rootDir, ".every-code-ai");
const aiSettingsPath = path.join(rootDir, "tools", "ai-workspace", "ai-settings.json");
const aiHome = existsSync(aiSettingsPath)
  ? (() => {
      try {
        const parsed = JSON.parse(readFileSync(aiSettingsPath, "utf8"));
        return String(parsed.codeHome ?? aiHomeDefault).replace(/%USERPROFILE%/g, process.env.USERPROFILE ?? "");
      } catch {
        return aiHomeDefault;
      }
    })()
  : aiHomeDefault;

const log = (msg) => console.log(`[ai:${command}] ${msg}`);
const fail = (msg) => {
  console.error(`[ai:${command}] ${msg}`);
  process.exit(1);
};

const command = process.argv[2] ?? "help";

function printHelp() {
  console.log(`AI workspace helpers

Usage:
  coder ai install   Confirm or install AI workspace subtree dependencies
  coder ai status    Print the workspace paths and expected service URLs
  coder ai smoke     Run a lightweight seam check against RAG + proxy
  coder ai dev       Start the AI services, then launch the Every Code CLI
  coder ai help      Show this message
  coder ai rag       Start the RAG GUI backend
  coder ai proxy     Start the OpenRouter proxy
  coder ai check     Build RAG and syntax-check the proxy

Environment:
  RAG_GUI_URL   Default: http://127.0.0.1:8787
  PROXY_BASE_URL Default: http://127.0.0.1:8080
  PROXY_PORT    Default: 8080
  PORT          Fallback for proxy port
  UI_PORT       Proxy UI port override when supported by the proxy package
  OPENROUTER_API_KEY / OPENROUTER_API_KEYS  Required for the proxy to start
  AI settings   tools/ai-workspace/ai-settings.json (generated from ai-settings.example.json)
  AI home       %USERPROFILE%\\.every-code-ai (Windows) or ~/.every-code-ai (other platforms)

Smoke check:
  Exercises the proxy code-assist preview path and fails loudly if the seam is not responding.
`);
}

function printStatus() {
  const state = (dir) => (existsSync(path.join(dir, "node_modules")) ? "installed" : "missing node_modules");
  console.log(`AI workspace root: ${rootDir}`);
  console.log(`RAG package: ${ragDir} (${state(ragDir)})`);
  console.log(`Proxy package: ${proxyDir} (${state(proxyDir)})`);
  console.log(`RAG GUI URL: ${ragGuiUrl}`);
  console.log(`Proxy base URL: ${proxyBaseUrl}`);
  console.log(`AI home: ${aiHome}`);
  console.log(`AI settings file: ${aiSettingsPath}`);
  console.log(`AI config: ${path.join(aiHome, "config.toml")} (local-proxy provider, generated at launch)`);
  console.log(`Proxy env: OPENROUTER_API_KEY or OPENROUTER_API_KEYS must be provided in the AI settings file or environment before start.`);
  console.log(`Expected flow: RAG GUI -> proxy code-assist -> OpenRouter fallback path`);
}

function run(cmd, args, cwd, extraEnv = {}) {
  const isWindowsCmd = process.platform === "win32" && typeof cmd === "string" && cmd.toLowerCase().endsWith(".cmd");
  const result = isWindowsCmd
    ? spawnSync("cmd.exe", ["/d", "/s", "/c", [cmd, ...args].join(" ")], {
        cwd,
        env: { ...process.env, ...extraEnv },
        stdio: "inherit",
        shell: false,
      })
    : spawnSync(cmd, args, {
        cwd,
        env: { ...process.env, ...extraEnv },
        stdio: "inherit",
        shell: false,
      });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

async function smoke() {
  const payload = {
    mode: "explain",
    task: "Explain this code path and summarize the key behavior.",
    currentCode: "function add(a, b) { return a + b; }",
    previewOnly: true,
    topK: 3,
    maxContextChars: 2000,
  };

  let resp;
  try {
    resp = await fetch(`${proxyBaseUrl}/api/code-assist`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(payload),
    });
  } catch (err) {
    fail(`Proxy seam is unreachable at ${proxyBaseUrl}/api/code-assist: ${err?.message || err}`);
  }

  if (!resp.ok) {
    const body = await resp.text().catch(() => "");
    fail(`Proxy seam returned HTTP ${resp.status} from ${proxyBaseUrl}/api/code-assist${body ? `: ${body}` : ""}`);
  }

  const data = await resp.json().catch(() => null);
  if (!data || typeof data !== "object") {
    fail("Proxy seam did not return JSON");
  }

  if (!data.prompt || !data.retrieval) {
    fail("Proxy preview response is missing prompt or retrieval payload");
  }

  console.log(`[ai:smoke] previewOnly response received from ${proxyBaseUrl}/api/code-assist`);
  console.log(`[ai:smoke] prompt length: ${String(data.prompt).length}`);
  console.log(`[ai:smoke] retrieval matches: ${Array.isArray(data.retrieval?.matches) ? data.retrieval.matches.length : 0}`);
}

switch (command) {
  case "help":
    printHelp();
    break;
  case "status":
    printStatus();
    break;
  case "rag":
    run(npmCmd, ["run", "gui:backend"], ragDir);
    break;
  case "proxy":
    run(npmCmd, ["start"], proxyDir);
    break;
  case "install": {
    const ragMods = path.join(ragDir, "node_modules");
    const proxyMods = path.join(proxyDir, "node_modules");
    if (existsSync(ragMods) && existsSync(proxyMods)) {
      console.log(`[ai:install] rag-mcp-server already has node_modules; skipping.`);
      console.log(`[ai:install] openrouter-proxy already has node_modules; skipping.`);
      console.log(`[ai:install] AI workspace dependencies are ready.`);
      break;
    }
    if (!existsSync(ragMods)) {
      log(`installing rag-mcp-server dependencies`);
      run(npmCmd, ["install"], ragDir);
    }
    if (!existsSync(proxyMods)) {
      log(`installing openrouter-proxy dependencies`);
      run(npmCmd, ["install"], proxyDir);
    }
    console.log(`[ai:install] AI workspace dependencies are ready.`);
    break;
  }
  case "smoke":
    await smoke();
    break;
  case "check":
    {
      const tscBin = path.join(ragDir, "node_modules", "typescript", "bin", "tsc");
      if (!existsSync(tscBin)) {
        fail(`Missing TypeScript compiler at ${tscBin}. Run ai:install first.`);
      }
      run(process.execPath, [tscBin, "-p", "tsconfig.json"], ragDir);
    }
    run(process.execPath, ["--check", path.join(proxyDir, "src", "server.js")], rootDir);
    console.log(`[ai:check] RAG build and proxy syntax check passed.`);
    break;
  default:
    fail(`Unknown command: ${command}`);
}
