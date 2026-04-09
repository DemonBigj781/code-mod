import express from 'express';
import rateLimit from 'express-rate-limit';
import helmet from 'helmet';
import morgan from 'morgan';
import dotenv from 'dotenv';
import http from 'http';
import { fetch } from 'undici';
import { pipeline } from 'stream/promises';
import { Readable } from 'stream';
import puter from '@heyputer/puter.js';

dotenv.config();

const {
  PORT = '8080',
  OPENROUTER_BASE_URL = 'https://openrouter.ai',
  OPENROUTER_API_KEY,
  OPENROUTER_API_KEYS,
  PROXY_SECRET,
  RATE_WINDOW_MS = 60000,
  RATE_MAX = 120,
  KEY_COOLDOWN_MS = 120000,
  REQUEST_TIMEOUT_MS = 120000,
  PROXY_PORT,
  UI_PORT = '3000',
  FREE_MODEL_REFRESH_MS = 300000,
  RAG_MCP_URL = 'http://127.0.0.1:8787',
  RAG_TOP_K = '6',
  RAG_SOURCE_KINDS = '',
  AIHORDE_API_KEY = '0000000000'
} = process.env;

const proxyPort = Number(PROXY_PORT ?? PORT);
const uiPort = Number(UI_PORT);
const freeModelRefreshMs = Number(FREE_MODEL_REFRESH_MS ?? 300000);
const anonymousFallbackCooldownMs = 60 * 60 * 1000;

if (process.env.PUTER_AUTH_TOKEN && typeof puter?.setAuthToken === 'function') {
  puter.setAuthToken(process.env.PUTER_AUTH_TOKEN);
}

let allowedModels = [];
let dynamicFreeModels = [];
let freeModelCatalog = [];
let lastWorkingModel = null;
let anonymousFallbackCooldownUntil = 0;
const failedModelCooldowns = new Map();
let lastFreeModelLogSignature = '';
const hiddenUiModels = new Set(['openrouter/free']);

function getDefaultFreeModel() {
  return getUiModels()[0] ?? null;
}

function getUiModels() {
  return allowedModels.filter(
    (model) => model && !hiddenUiModels.has(model.toLowerCase())
  );
}

const configuredKeys = (
  (OPENROUTER_API_KEY ?? OPENROUTER_API_KEYS ?? '')
    .split(',')
    .map((key) => key.trim())
    .filter((key) => key && !key.toUpperCase().includes('EXAMPLE'))
);

if (!configuredKeys.length) {
  console.error('Missing OPENROUTER_API_KEY or OPENROUTER_API_KEYS; proxy cannot start.');
  process.exit(1);
}

async function refreshDynamicFreeModels() {
  if (!configuredKeys.length) {
    return;
  }

  const keysToTry = getApiKeysInRotationOrder();

  for (const refreshKey of keysToTry) {
    try {
      const response = await fetch(`${OPENROUTER_BASE_URL}/api/v1/models`, {
        headers: {
          Authorization: `Bearer ${refreshKey}`
        }
      });

      if (!response.ok) {
        penalizeKeyIfNeeded(refreshKey, response.status);
        console.warn(
          `Unable to refresh free model list with configured key: ${response.status} ${response.statusText}`
        );
        continue;
      }

      const payload = await response.json();
      const entries = payload?.models ?? payload?.data ?? payload ?? [];
      const freeModels = [...new Set((entries ?? [])
        .map((entry) => extractFreeModelInfo(entry))
        .filter((entry) => entry && entry.id))];

      freeModels.sort((left, right) => {
        const leftContext = left.contextLength ?? 0;
        const rightContext = right.contextLength ?? 0;
        if (rightContext !== leftContext) {
          return rightContext - leftContext;
        }
        return left.id.localeCompare(right.id);
      });

      freeModelCatalog = freeModels;
      dynamicFreeModels = freeModels.map((entry) => entry.id);
      allowedModels = [...dynamicFreeModels];

      const nextSignature = dynamicFreeModels.join('|');

      if (freeModels.length && nextSignature !== lastFreeModelLogSignature) {
        console.log(
          'Discovered free models:',
          freeModels
            .slice(0, 10)
            .map((entry) => `${entry.id} (${entry.contextLength ?? 'unknown'})`)
            .join(', ')
        );
        lastFreeModelLogSignature = nextSignature;
      }
      return { freeModels: [...dynamicFreeModels], error: null };
    } catch (error) {
      console.warn('Failed to refresh free models with configured key:', error.message);
    }
  }

  return { freeModels: [...dynamicFreeModels], error: new Error('Unable to refresh free model list with any configured API key') };
}

refreshDynamicFreeModels();

if (freeModelRefreshMs > 0) {
  setInterval(refreshDynamicFreeModels, freeModelRefreshMs);
}

const keyCooldowns = new Map();
let currentKeyIndex = 0;

const app = express();

app.use(express.raw({ type: () => true, limit: '10mb' }));
app.use(helmet());
app.use(morgan('combined'));

const limiter = rateLimit({
  windowMs: Number(RATE_WINDOW_MS),
  max: Number(RATE_MAX),
  standardHeaders: true,
  legacyHeaders: false
});

const authenticateClient = (req, res, next) => {
  if (!PROXY_SECRET) {
    return next();
  }

  if (req.method === 'OPTIONS') {
    return next();
  }

  if (isTrustedUiOrigin(req) || isLoopbackRequest(req)) {
    return next();
  }

  const sentKey =
    req.headers['authorization']?.split(' ')[1] ??
    req.headers['x-api-key'] ??
    req.query?.apiKey ??
    req.query?.key;

  if (sentKey === PROXY_SECRET) {
    return next();
  }

  res.status(401).json({ error: 'Unauthorized proxy client' });
};

app.get('/health', (_req, res) => {
  res.setHeader('Access-Control-Allow-Origin', '*');
  res.status(200).json({
    status: 'ok',
    listeners: configuredKeys.length,
    version: '0.1.0'
  });
});

app.use(['/api', '/v1'], limiter, authenticateClient);

app.options(['/api/*', '/v1/*'], (req, res) => {
  applyApiCorsHeaders(req, res);
  res.sendStatus(204);
});

app.get('/models', async (_req, res) => {
  res.setHeader('Access-Control-Allow-Origin', '*');
  await refreshDynamicFreeModels();
  res.status(200).json({
    allowedModels,
    uiModels: getUiModels(),
    dynamicFreeModels
  });
});

app.get('/v1/models', async (_req, res) => {
  res.setHeader('Access-Control-Allow-Origin', '*');
  await refreshDynamicFreeModels();
  res.status(200).json({
    object: 'list',
    data: freeModelCatalog.map((entry) => ({
      id: entry.id,
      object: 'model',
      created: entry.created ?? 0,
      owned_by: entry.ownedBy ?? 'openrouter',
      context_length: entry.contextLength ?? null
    }))
  });
});

app.post(['/api/code-assist', '/v1/code-assist'], async (req, res, next) => {
  applyApiCorsHeaders(req, res);

  try {
    const parsedBody = parseJsonBody(req.body);
    if (parsedBody.error) {
      return res.status(400).json({ error: parsedBody.error });
    }

    const requestedModel = parsedBody.value?.model ?? req.query?.model;
    const codeAssistPayload = await buildCodeAssistPayload(parsedBody.value);
    if (codeAssistPayload.error) {
      return res.status(400).json({ error: codeAssistPayload.error });
    }

    if (codeAssistPayload.previewOnly) {
      return res.status(200).json({
        mode: codeAssistPayload.mode,
        model: codeAssistPayload.model,
        previewOnly: true,
        query: codeAssistPayload.query,
        retrieval: codeAssistPayload.retrieval,
        prompt: codeAssistPayload.prompt,
        messages: codeAssistPayload.messages
      });
    }

    const targetUrl = new URL('/v1/chat/completions', OPENROUTER_BASE_URL);
    const chosenKey = selectApiKey();
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), Number(REQUEST_TIMEOUT_MS));
    const baseHeaders = {
      ...filterHeaders(req.headers),
      'x-forwarded-host': req.headers.host,
      'x-forwarded-for': [req.headers['x-forwarded-for'], req.ip].filter(Boolean).join(', ')
    };

    const terminalResult = await tryModelsSequentially({
      targetUrl,
      req,
      modelCandidates: buildModelCandidates(requestedModel ?? codeAssistPayload.model),
      baseHeaders,
      chosenKey,
      controller,
      body: codeAssistPayload.body,
      parsedJson: codeAssistPayload.parsedJson
    });

    clearTimeout(timeout);

    if (terminalResult?.matchedModel) {
      lastWorkingModel = terminalResult.matchedModel;
    }

    if (terminalResult?.syntheticJson) {
      res.status(200);
      res.setHeader('content-type', 'application/json; charset=utf-8');
      res.setHeader('cache-control', 'no-store');
      res.send(JSON.stringify(terminalResult.syntheticJson));
      return;
    }

    if (terminalResult.error) {
      return res.status(502).json({
        error: {
          message: terminalResult.error,
          type: 'invalid_request_error',
          param: null,
          code: null
        }
      });
    }

    const { upstream } = terminalResult;
    const finalHeaders = upstream.headers;
    const contentType = finalHeaders.get('content-type') ?? '';
    const isJsonResponse = contentType.includes('application/json');
    const isStreamResponse = contentType.includes('text/event-stream');

    if (isJsonResponse && !isStreamResponse) {
      const upstreamJson = await upstream.clone().json().catch(() => null);
      const normalized = normalizeOpenAiChatCompletion(upstreamJson, codeAssistPayload.model ?? requestedModel);
      if (normalized) {
        res.status(upstream.status);
        res.setHeader('content-type', 'application/json; charset=utf-8');
        res.setHeader('cache-control', 'no-store');
        res.send(JSON.stringify(normalized));
        return;
      }
    }

    res.status(upstream.status);
    finalHeaders.forEach((value, name) => {
      if (name.toLowerCase() === 'transfer-encoding') {
        return;
      }
      res.setHeader(name, value);
    });

    const upstreamStream = upstream.body ? Readable.fromWeb(upstream.body) : null;
    if (upstreamStream) {
      upstreamStream.on('error', (streamError) => {
        if (isIgnorableStreamError(streamError)) {
          return;
        }
        console.warn('[proxy->openrouter] upstream stream error:', streamError.message);
      });
      res.flushHeaders();
      await pipeline(upstreamStream, res);
    } else {
      res.end();
    }
  } catch (error) {
    if (isIgnorableStreamError(error) || res.headersSent || res.writableEnded) {
      return;
    }
    next(error);
  }
});

app.all(['/api/*', '/v1/*'], async (req, res, next) => {
  applyApiCorsHeaders(req, res);
  const targetPath = toOpenRouterPath(req.originalUrl);
  const targetUrl = new URL(targetPath, OPENROUTER_BASE_URL);
  const chosenKey = selectApiKey();
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), Number(REQUEST_TIMEOUT_MS));

  let body =
    ['GET', 'HEAD'].includes(req.method) || !req.body || !req.body.length
      ? undefined
      : req.body;

  const { sanitizedBody, error, parsedJson } = enforceFreeModel(
    req,
    body
  );

  if (error) {
    clearTimeout(timeout);
    return res.status(400).json({ error });
  }

  if (sanitizedBody) {
    body = sanitizedBody;
  }

  const headers = filterHeaders(req.headers);

  try {
    const requestedModel = parsedJson?.model ?? req.query?.model;
    const uiModels = getUiModels();
    if (!uiModels.length) {
      clearTimeout(timeout);
      return res.status(400).json({
        error: 'No eligible free models are available after filtering the OpenRouter model list.'
      });
    }
    const modelCandidates = buildModelCandidates(requestedModel);
    const baseHeaders = {
      ...headers,
      'x-forwarded-host': req.headers.host ?? headers['x-forwarded-host'],
      'x-forwarded-for': [
        req.headers['x-forwarded-for'],
        req.ip
      ]
        .filter(Boolean)
        .join(', ')
    };

    const terminalResult = await tryModelsSequentially({
      targetUrl,
      req,
      modelCandidates,
      baseHeaders,
      chosenKey,
      controller,
      body,
      parsedJson
    });

    if (terminalResult?.matchedModel) {
      lastWorkingModel = terminalResult.matchedModel;
    }

    if (terminalResult?.syntheticJson) {
      res.status(200);
      res.setHeader('content-type', 'application/json; charset=utf-8');
      res.setHeader('cache-control', 'no-store');
      res.send(JSON.stringify(terminalResult.syntheticJson));
      return;
    }

    if (terminalResult.error) {
      return res.status(502).json({
        error: {
          message: terminalResult.error,
          type: 'invalid_request_error',
          param: null,
          code: null
        }
      });
    }

    const { upstream } = terminalResult;
    const finalHeaders = upstream.headers;
    const contentType = finalHeaders.get('content-type') ?? '';
    const isJsonResponse = contentType.includes('application/json');
    const isStreamResponse = contentType.includes('text/event-stream');

    if (isJsonResponse && !isStreamResponse) {
      const upstreamJson = await upstream.clone().json().catch(() => null);
      const normalized = normalizeOpenAiChatCompletion(upstreamJson, parsedJson?.model ?? requestedModel);
      if (normalized) {
        res.status(upstream.status);
        res.setHeader('content-type', 'application/json; charset=utf-8');
        res.setHeader('cache-control', 'no-store');
        res.send(JSON.stringify(normalized));
        return;
      }
    }

    res.status(upstream.status);
    finalHeaders.forEach((value, name) => {
      if (name.toLowerCase() === 'transfer-encoding') {
        return;
      }
      res.setHeader(name, value);
    });
    const upstreamStream = upstream.body
      ? Readable.fromWeb(upstream.body)
      : null;

    if (upstreamStream) {
      upstreamStream.on('error', (streamError) => {
        if (isIgnorableStreamError(streamError)) {
          return;
        }
        console.warn('[proxy->openrouter] upstream stream error:', streamError.message);
      });
      res.flushHeaders();
      await pipeline(upstreamStream, res);
    } else {
      res.end();
    }
  } catch (error) {
    if (isIgnorableStreamError(error) || res.headersSent || res.writableEnded) {
      return;
    }
    if (error.name === 'AbortError') {
      return res.status(504).json({
        error: {
          message: 'Request timed out contacting OpenRouter',
          type: 'timeout_error',
          param: null,
          code: null
        }
      });
    }
    next(error);
  } finally {
    clearTimeout(timeout);
  }
});

app.use((err, _req, res, _next) => {
  console.error('proxy error', err.stack || err.message);
  if (res.headersSent) {
    return;
  }
  res.status(500).json({ error: 'Proxy failure', details: err.message });
});

const port = proxyPort;
const proxyServer = app.listen(port, () => {
  console.log(`OpenRouter proxy listening on http://localhost:${port}`);
});
proxyServer.on('error', (error) => {
  if (error?.code === 'EADDRINUSE') {
    console.error(`Proxy port ${port} is already in use. Another proxy instance is likely running.`);
    process.exit(1);
  }
  throw error;
});

const uiApp = express();
uiApp.use(express.raw({ type: () => true, limit: '10mb' }));

uiApp.get('/health', forwardUiRequest);
uiApp.get('/models', forwardUiRequest);
uiApp.get('/v1/models', forwardUiRequest);
uiApp.all(['/api/*', '/v1/*'], forwardUiRequest);

uiApp.get('/', (req, res) => {
  res.setHeader('Cache-Control', 'no-store');
  const host = (req.headers.host ?? 'localhost').split(':')[0];
  const healthUrl = '/health';
  const chatUrl = '/v1/chat/completions';
  const allowedModelsJson = JSON.stringify(allowedModels);
  const uiModelsJson = JSON.stringify(getUiModels());
  const uiDefaultModel = getDefaultFreeModel();
  res.setHeader('Content-Type', 'text/html; charset=utf-8');
  res.send(`<!doctype html>
<html>
  <head>
    <title>OpenRouter Proxy Monitor</title>
    <style>
      body { font-family: system-ui, sans-serif; background:#0f172a; color:#f1f5f9; margin:0; min-height:100vh; display:flex; align-items:center; justify-content:center; }
      .card { background:#1e293b; padding:2rem; border-radius:1rem; box-shadow:0 10px 30px rgba(15,23,42,0.4); width:min(90vw,640px); }
      .chat { margin-top:1.5rem; border:1px solid #334155; border-radius:0.75rem; padding:1rem; background:#0f172a; }
      .chat-log { max-height:260px; overflow:auto; padding:0.5rem; border:1px solid #1f2937; border-radius:0.5rem; background:#020617; margin-bottom:0.5rem; }
      .chat-message { margin:0.25rem 0; }
      .chat-message { white-space: pre-wrap; }
      .chat-message strong { color:#a3e635; }
      .reply-box { margin-top:0.75rem; padding:0.75rem; border:1px solid #334155; border-radius:0.5rem; background:#020617; white-space:pre-wrap; min-height:3rem; }
      form { display:flex; gap:0.5rem; }
      form input { flex:1; padding:0.5rem 0.75rem; border-radius:0.5rem; border:1px solid #334155; background:#020617; color:#fff; }
      form button { flex:none; padding:0.55rem 1.25rem; border-radius:0.5rem; background:#2563eb; color:white; border:none; cursor:pointer; }
      pre { background:#0b1120; padding:1rem; border-radius:0.75rem; overflow:auto; }
      button { margin-top:1rem; padding:0.75rem 1.5rem; border:none; border-radius:999px; background:#2563eb; color:white; cursor:pointer; font-size:1rem; }
    </style>
  </head>
  <body>
    <div class="card">
      <h1>OpenRouter Proxy UI</h1>
      <p>Proxy port: <strong>${proxyPort}</strong></p>
      <p>Allowed free models: <strong id="model-summary">${getUiModels().join(', ')}</strong></p>
      <button id="refresh">Refresh health</button>
      <div id="health" aria-live="polite" style="margin-top:1rem;"></div>
      <div class="chat">
        <h2>Text chat</h2>
        <div id="chat-log" class="chat-log" aria-live="polite"></div>
        <form id="chat-form">
          <input id="chat-input" type="text" placeholder="Ask a question..." autocomplete="off" required />
          <button type="submit">Send</button>
        </form>
        <div style="margin-top:0.75rem;">
          <strong>Latest response</strong>
          <div id="latest-reply" class="reply-box" aria-live="polite"></div>
        </div>
      </div>
    </div>
      <script>
        const healthEl = document.getElementById('health');
        const refresh = document.getElementById('refresh');
        const chatLog = document.getElementById('chat-log');
        const chatForm = document.getElementById('chat-form');
        const chatInput = document.getElementById('chat-input');
        const modelSummary = document.getElementById('model-summary');
        const latestReply = document.getElementById('latest-reply');
        const allowedModels = ${allowedModelsJson};
      const uiModels = ${uiModelsJson};
      let defaultModel = 'free';
  const modelsEndpoint = '/v1/models';
  const chatUrl = '/v1/chat/completions';

      function appendMessage(role, text) {
        const div = document.createElement('p');
        div.className = 'chat-message';
        div.textContent = role + ': ' + text;
        chatLog.appendChild(div);
        chatLog.scrollTop = chatLog.scrollHeight;
      }

      async function sendChat(message) {
        if (!defaultModel) {
          appendMessage('Bot', 'No eligible free models are available.');
          return;
        }
        appendMessage('You', message);
        const payload = {
          model: defaultModel,
          messages: [{ role: 'user', content: message }],
          stream: false
        };
        try {
          const response = await fetch(chatUrl, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(payload)
          });
          const raw = await response.text();
          let json = null;
          try {
            json = raw ? JSON.parse(raw) : null;
          } catch {
            json = null;
          }
          const reply =
            json?.choices?.[0]?.message?.content ??
            json?.error?.message ??
            (typeof json?.error === 'string' ? json.error : null) ??
            raw?.trim() ??
            'No response received';
          appendMessage('Bot', reply);
          latestReply.textContent = reply;
        } catch (err) {
          appendMessage('Bot', 'Request failed: ' + err.message);
          latestReply.textContent = 'Request failed';
        }
      }

      chatForm.addEventListener('submit', async (event) => {
        event.preventDefault();
        const text = chatInput.value.trim();
        if (!text) return;
        chatInput.value = '';
        chatInput.disabled = true;
        await sendChat(text);
        chatInput.disabled = false;
        chatInput.focus();
      });

      async function loadHealth() {
        refresh.disabled = true;
        try {
          const res = await fetch('${healthUrl}', { cache: 'no-store' });
          const body = await res.json().catch(() => ({}));
          healthEl.innerHTML = '<pre>Status ' + res.status + '\\n' + JSON.stringify(body, null, 2) + '</pre>';
        } catch (err) {
          healthEl.innerHTML = '<pre>Health check failed: ' + err.message + '</pre>';
        } finally {
          refresh.disabled = false;
        }
      }
      async function loadModels() {
        try {
          const res = await fetch(modelsEndpoint, { cache: 'no-store' });
          const json = await res.json().catch(() => null);
          const list = (json?.data ?? json?.uiModels ?? []).map((model) => {
            if (typeof model === 'string') {
              return model;
            }
            return model?.id;
          }).filter(Boolean).filter((model) => model.toLowerCase() !== 'openrouter/free');
          modelSummary.textContent = list.join(', ') || 'none';
          defaultModel = list.length ? 'free' : null;
          if (list.length === 0) {
            appendMessage('Bot', 'No eligible free models are available.');
          }
        } catch (err) {
          modelSummary.textContent = 'error loading';
        }
      }

      refresh.addEventListener('click', loadHealth);
      loadHealth();
      loadModels();
    </script>
  </body>
</html>`);
});

const uiServer = uiApp.listen(uiPort, () => {
  console.log(`UI available at http://localhost:${uiPort}`);
});
uiServer.on('error', (error) => {
  if (error?.code === 'EADDRINUSE') {
    console.error(`UI port ${uiPort} is already in use. Another UI instance is likely running.`);
    process.exit(1);
  }
  throw error;
});

async function forwardUiRequest(req, res, next) {
  try {
    const upstreamUrl = `http://localhost:${proxyPort}${req.originalUrl}`;
    const headers = filterHeaders(req.headers);

    if (PROXY_SECRET) {
      headers.authorization = `Bearer ${PROXY_SECRET}`;
    }

    logCurlRequest('ui->proxy', {
      method: req.method,
      url: upstreamUrl,
      headers,
      body: ['GET', 'HEAD'].includes(req.method) ? undefined : req.body
    });

    const bodyBuffer =
      ['GET', 'HEAD'].includes(req.method) || !req.body?.length ? null : Buffer.from(req.body);

    const upstream = await forwardHttpRequest({
      method: req.method,
      port: proxyPort,
      path: req.originalUrl,
      headers,
      body: bodyBuffer
    });

    console.log(`[ui->proxy] ${req.method} ${upstreamUrl} -> ${upstream.statusCode}`);

    res.status(upstream.statusCode);
    Object.entries(upstream.headers).forEach(([name, value]) => {
      const normalized = name.toLowerCase();
      if (normalized === 'transfer-encoding' || normalized === 'content-length' || normalized === 'content-encoding') {
        return;
      }
      if (Array.isArray(value)) {
        res.setHeader(name, value.join(', '));
      } else if (value != null) {
        res.setHeader(name, value);
      }
    });

    if (!upstream.body.length) {
      res.end();
      return;
    }

    res.setHeader('content-length', String(upstream.body.length));
    res.send(upstream.body);
  } catch (error) {
    if (isIgnorableStreamError(error) || res.headersSent || res.writableEnded) {
      return;
    }
    next(error);
  }
}

function selectApiKey() {
  const now = Date.now();
  let earliestBlocked = null;

  for (let i = 0; i < configuredKeys.length; i += 1) {
    const candidateIndex = (currentKeyIndex + i) % configuredKeys.length;
    const candidateKey = configuredKeys[candidateIndex];
    const blockedUntil = keyCooldowns.get(candidateKey);

    if (!blockedUntil || blockedUntil <= now) {
      currentKeyIndex = (candidateIndex + 1) % configuredKeys.length;
      return candidateKey;
    }

    if (!earliestBlocked || blockedUntil < earliestBlocked.blockedUntil) {
      earliestBlocked = {
        key: candidateKey,
        index: candidateIndex,
        blockedUntil
      };
    }
  }

  if (earliestBlocked) {
    console.warn(
      `[proxy->openrouter] all configured API keys are on cooldown; reusing the next key to recover at ${new Date(earliestBlocked.blockedUntil).toISOString()}`
    );
    currentKeyIndex = (earliestBlocked.index + 1) % configuredKeys.length;
    return earliestBlocked.key;
  }

  const fallback = configuredKeys[currentKeyIndex];
  currentKeyIndex = (currentKeyIndex + 1) % configuredKeys.length;
  return fallback;
}

function getApiKeysInRotationOrder() {
  const now = Date.now();
  const available = [];
  const blocked = [];
  const seen = new Set();

  for (let i = 0; i < configuredKeys.length; i += 1) {
    const candidateIndex = (currentKeyIndex + i) % configuredKeys.length;
    const candidateKey = configuredKeys[candidateIndex];
    if (seen.has(candidateKey)) {
      continue;
    }
    seen.add(candidateKey);

    const blockedUntil = keyCooldowns.get(candidateKey);
    if (!blockedUntil || blockedUntil <= now) {
      available.push(candidateKey);
    } else {
      blocked.push({ key: candidateKey, blockedUntil, index: candidateIndex });
    }
  }

  if (available.length) {
    return available;
  }

  blocked.sort((left, right) => left.blockedUntil - right.blockedUntil || left.index - right.index);
  if (blocked.length) {
    console.warn(
      `[proxy->openrouter] all configured API keys are on cooldown for free-model refresh; trying the earliest recovery key at ${new Date(blocked[0].blockedUntil).toISOString()}`
    );
    return blocked.map((entry) => entry.key);
  }

  return [...configuredKeys];
}

function enforceFreeModel(req, bodyBuffer) {
  const contentType = req.headers['content-type'] ?? '';
  const queryModel = req.query?.model;

  if (queryModel && !isModelAllowed(queryModel)) {
    return {
      error: `Model '${queryModel}' is not permitted; the proxy allows only free models (${allowedModels.join(
        ', '
      )}).`
    };
  }

  if (!bodyBuffer || !contentType.includes('application/json')) {
    return { sanitizedBody: bodyBuffer };
  }

  try {
    const payload = JSON.parse(bodyBuffer.toString('utf-8'));
    const bodyModel = payload?.model;

    if (bodyModel) {
      if (!isModelAllowed(bodyModel)) {
        return {
          error: `Model '${bodyModel}' is not permitted; the proxy allows only free models (${allowedModels.join(
            ', '
          )}).`
        };
      }
    } else if (!queryModel) {
      payload.model = getDefaultFreeModel();
    }

    const serialized = JSON.stringify(payload);
  return {
    sanitizedBody: Buffer.from(serialized, 'utf-8'),
    parsedJson: payload
  };
  } catch (err) {
    console.warn('Failed to inspect request body for model enforcement:', err.message);
    return { sanitizedBody: bodyBuffer };
  }
}

function isModelAllowed(model) {
  if (!model) {
    return false;
  }

  if (model.toLowerCase() === 'free') {
    return true;
  }

  if (allowedModels.includes(model)) {
    return true;
  }

  return model.toLowerCase().endsWith(':free');
}

function extractFreeModelId(entry) {
  if (!entry) {
    return null;
  }

  const candidates = [
    typeof entry === 'string' ? entry : entry.id,
    typeof entry === 'string' ? null : entry.name
  ];

  for (const candidate of candidates) {
    if (typeof candidate === 'string' && candidate.toLowerCase().includes(':free')) {
      return candidate.trim();
    }
  }

  return null;
}

function extractFreeModelInfo(entry) {
  const id = extractFreeModelId(entry);
  if (!id) {
    return null;
  }

  const contextLength = Number(
    entry?.context_length ?? entry?.contextLength ?? entry?.context?.length ?? 0
  );

  return {
    id,
    contextLength: Number.isFinite(contextLength) ? contextLength : 0,
    name: typeof entry?.name === 'string' ? entry.name : null
  };
}

function penalizeKeyIfNeeded(key, status) {
  if ([401, 429].includes(status)) {
    keyCooldowns.set(key, Date.now() + Number(KEY_COOLDOWN_MS));
  }
}

function filterHeaders(originalHeaders) {
  const excluded = new Set([
    'host',
    'content-length',
    'transfer-encoding',
    'expect',
    'connection',
    'accept-encoding'
  ]);

  return Object.entries(originalHeaders).reduce((acc, [key, value]) => {
    if (excluded.has(key.toLowerCase())) {
      return acc;
    }
    if (value) {
      acc[key] = value;
    }
    return acc;
  }, {});
}

function logCurlRequest(label, { method, url, headers, body }) {
  const parts = [`curl -i -X ${method} "${url}"`];

  for (const [key, value] of Object.entries(headers ?? {})) {
    if (value == null || value === '') {
      continue;
    }

    const normalized = key.toLowerCase();
    const renderedValue =
      normalized === 'authorization' || normalized === 'x-api-key'
        ? '[redacted]'
        : escapeForCurl(value);

    parts.push(`-H "${key}: ${renderedValue}"`);
  }

  if (!['GET', 'HEAD'].includes(method) && body && body.length) {
    const preview = Buffer.isBuffer(body) ? body.toString('utf8') : String(body);
    parts.push(`--data-raw "${escapeForCurl(preview.slice(0, 800))}"`);
  }

  console.log(`[${label}] ${parts.join(' ')}`);
}

function escapeForCurl(value) {
  return String(value)
    .replace(/\\/g, '\\\\')
    .replace(/"/g, '\\"')
    .replace(/\r/g, '\\r')
    .replace(/\n/g, '\\n');
}

function isTrustedUiOrigin(req) {
  const origin = req.headers.origin;
  if (!origin) {
    return false;
  }

  try {
    const originUrl = new URL(origin);
    return (
      originUrl.hostname === 'localhost' ||
      originUrl.hostname === '127.0.0.1'
    );
  } catch {
    return false;
  }
}

function isLoopbackRequest(req) {
  const candidates = [
    req.socket?.remoteAddress,
    req.ip,
    req.headers['x-forwarded-for']?.split(',')[0]?.trim()
  ];

  return candidates.some((candidate) => {
    if (!candidate) {
      return false;
    }

    const normalized = candidate.replace('::ffff:', '');
    return (
      normalized === '127.0.0.1' ||
      normalized === '::1' ||
      normalized === 'localhost'
    );
  });
}

function isIgnorableStreamError(error) {
  return (
    error?.code === 'ERR_STREAM_PREMATURE_CLOSE' ||
    error?.code === 'ERR_STREAM_DESTROYED' ||
    error?.name === 'AbortError' ||
    String(error?.message ?? '').toLowerCase().includes('terminated')
  );
}

function applyApiCorsHeaders(req, res) {
  const origin = req.headers.origin;
  if (!origin) {
    return;
  }

  res.setHeader('Access-Control-Allow-Origin', origin);
  res.setHeader('Vary', 'Origin');
  res.setHeader('Access-Control-Allow-Methods', 'GET,POST,OPTIONS');
  res.setHeader('Access-Control-Allow-Headers', 'Content-Type, Authorization, X-API-Key');
  res.setHeader('Access-Control-Max-Age', '86400');
}

function toOpenRouterPath(originalUrl) {
  if (originalUrl === '/v1') {
    return '/api/v1';
  }
  if (originalUrl.startsWith('/v1/')) {
    return `/api${originalUrl}`;
  }
  return originalUrl;
}

function normalizeOpenAiChatCompletion(payload, requestedModel) {
  const choice = payload?.choices?.[0] ?? null;
  const message = choice?.message ?? null;
  const content = message?.content ?? choice?.text ?? '';

  if (!payload || (!choice && !message && content === '')) {
    return null;
  }

  const created = Number(payload.created ?? Math.floor(Date.now() / 1000));
  const model = payload.model ?? requestedModel ?? null;
  const normalized = {
    id: payload.id ?? `chatcmpl-${created}`,
    object: payload.object === 'chat.completion' ? 'chat.completion' : 'chat.completion',
    created,
    model,
    choices: [
      {
        index: choice?.index ?? 0,
        logprobs: choice?.logprobs ?? null,
        message: {
          role: message?.role ?? 'assistant',
          content
        },
        finish_reason: choice?.finish_reason ?? null
      }
    ]
  };

  if (payload.usage) {
    normalized.usage = {
      prompt_tokens: payload.usage.prompt_tokens ?? 0,
      completion_tokens: payload.usage.completion_tokens ?? 0,
      total_tokens: payload.usage.total_tokens ?? 0
    };
  }

  if (payload.system_fingerprint !== undefined) {
    normalized.system_fingerprint = payload.system_fingerprint;
  }

  return normalized;
}

function forwardHttpRequest({ method, port, path, headers, body }) {
  return new Promise((resolve, reject) => {
    const request = http.request(
      {
        hostname: 'localhost',
        port,
        path,
        method,
        headers
      },
      (response) => {
        const chunks = [];
        response.on('data', (chunk) => chunks.push(chunk));
        response.on('end', () => {
          resolve({
            statusCode: response.statusCode ?? 500,
            headers: response.headers,
            body: Buffer.concat(chunks)
          });
        });
      }
    );

    request.on('error', reject);

    if (body?.length) {
      request.write(body);
    }

    request.end();
  });
}

function buildModelCandidates(requestedModel) {
  const normalized = requestedModel?.trim();
  const candidates = [];
  const seen = new Set();

  const pushCandidate = (model) => {
    if (!model || seen.has(model)) {
      return;
    }
    seen.add(model);
    candidates.push(model);
  };

  if (normalized && normalized.toLowerCase() !== 'free') {
    pushCandidate(normalized);
  }

  if (lastWorkingModel && !candidates.includes(lastWorkingModel)) {
    pushCandidate(lastWorkingModel);
  }

  for (const model of getUiModels()) {
    pushCandidate(model);
  }

  if (!candidates.length) {
    pushCandidate(getDefaultFreeModel());
  }

  return candidates.filter((model) => {
    const retryAt = failedModelCooldowns.get(model);
    return !retryAt || Date.now() >= retryAt;
  });
}

async function tryModelsSequentially({
  targetUrl,
  req,
  modelCandidates,
  baseHeaders,
  chosenKey,
  controller,
  body,
  parsedJson
}) {
  let lastError = null;
  let lastStatus = null;
  const shouldRetryOpenRouter = Date.now() >= anonymousFallbackCooldownUntil;

  if (shouldRetryOpenRouter) {
    for (const model of modelCandidates) {
      const attemptBody = buildBodyForModel(parsedJson, body, model);
      const attemptHeaders = {
        ...baseHeaders,
        authorization: `Bearer ${chosenKey}`
      };

      if (attemptBody) {
        attemptHeaders['content-length'] = String(attemptBody.length);
      }

      logCurlRequest('proxy->openrouter', {
        method: req.method,
        url: targetUrl.toString(),
        headers: attemptHeaders,
        body: attemptBody
      });

      console.log(`[proxy->openrouter] attempting model=${model}`);

      try {
        const upstream = await fetch(targetUrl.toString(), {
          method: req.method,
          headers: attemptHeaders,
          body: attemptBody,
          signal: controller.signal
        });

        penalizeKeyIfNeeded(chosenKey, upstream.status);

        console.log(
          `[proxy->openrouter] ${req.method} ${targetUrl.toString()} model=${model} -> ${upstream.status}`
        );

        if (shouldRetryStatus(upstream.status)) {
          failedModelCooldowns.set(model, Date.now() + anonymousFallbackCooldownMs);
        } else {
          failedModelCooldowns.delete(model);
        }

        if (upstream.ok) {
          const preview = await upstream.clone().text().catch(() => '');
          console.log(
            `[proxy->openrouter] response content-type=${upstream.headers.get('content-type') ?? 'unknown'} preview=${JSON.stringify(preview.slice(0, 500))}`
          );
        }

        if (!shouldRetryStatus(upstream.status)) {
          return { upstream, matchedModel: model };
        }

        lastStatus = upstream.status;
        await upstream.body?.cancel?.();
      } catch (error) {
        if (error.name === 'AbortError') {
          throw error;
        }
        lastError = error;
      }
    }
  }

  const puterResult = await tryPuterFallback(parsedJson);
  if (puterResult) {
    return puterResult;
  }

  if (AIHORDE_API_KEY && AIHORDE_API_KEY !== chosenKey) {
    if (!shouldRetryOpenRouter) {
      console.log(
        `[proxy->openrouter] retrying OpenRouter remains paused until ${new Date(anonymousFallbackCooldownUntil).toISOString()}, using anonymous fallback only`
      );
    }

    // Reference for this lowest-resort fallback path:
    // https://docs.puter.com/getting-started/
    // https://docs.puter.com/llms.txt
    // https://github.com/HeyPuter/vanilla.js
    const anonymousResult = await tryAnonymousFallback({
      targetUrl,
      req,
      baseHeaders,
      controller,
      body
    });

    if (anonymousResult?.upstream) {
      return anonymousResult;
    }
  }

  const message = lastError?.message ?? `All free models returned ${lastStatus || 'errors'}`;
  return { error: message };
}

async function tryPuterFallback(parsedJson) {
  if (!puter?.ai?.chat) {
    return null;
  }

  const messages = Array.isArray(parsedJson?.messages) ? parsedJson.messages : null;
  if (!messages?.length) {
    return null;
  }

  console.log('[proxy->openrouter] attempting Puter fallback');

  try {
    const completion = await puter.ai.chat(messages, {
      stream: false
    });
    const content =
      typeof completion === 'string'
        ? completion
        : completion?.message?.content ?? completion?.toString?.() ?? '';

    if (!content) {
      return null;
    }

    return {
      syntheticJson: buildOpenAiChatCompletion(content, parsedJson?.model ?? lastWorkingModel ?? 'puter')
    };
  } catch (error) {
    console.warn('[proxy->openrouter] Puter fallback failed:', error.message);
    return null;
  }
}

async function tryAnonymousFallback({
  targetUrl,
  req,
  baseHeaders,
  controller,
  body
}) {
  const attemptHeaders = {
    ...baseHeaders,
    authorization: `Bearer ${AIHORDE_API_KEY}`
  };

  if (body?.length) {
    attemptHeaders['content-length'] = String(body.length);
  }

  logCurlRequest('proxy->openrouter', {
    method: req.method,
    url: targetUrl.toString(),
    headers: attemptHeaders,
    body
  });

  console.log('[proxy->openrouter] attempting anonymous 0000000000 fallback');

  try {
    const upstream = await fetch(targetUrl.toString(), {
      method: req.method,
      headers: attemptHeaders,
      body,
      signal: controller.signal
    });

    console.log(
      `[proxy->openrouter] ${req.method} ${targetUrl.toString()} anonymous=0000000000 -> ${upstream.status}`
    );
    anonymousFallbackCooldownUntil = Date.now() + anonymousFallbackCooldownMs;

    if (upstream.ok || !shouldRetryStatus(upstream.status)) {
      return { upstream, matchedModel: lastWorkingModel ?? null };
    }

    await upstream.body?.cancel?.();
  } catch (error) {
    if (error.name === 'AbortError') {
      throw error;
    }
  }

  return null;
}

function buildOpenAiChatCompletion(content, model) {
  return {
    id: `chatcmpl-${Date.now()}`,
    object: 'chat.completion',
    created: Math.floor(Date.now() / 1000),
    model,
    choices: [
      {
        index: 0,
        message: {
          role: 'assistant',
          content
        },
        finish_reason: 'stop'
      }
    ]
  };
}

function buildBodyForModel(parsedJson, fallbackBody, model) {
  if (!parsedJson) {
    return fallbackBody;
  }

  const clonedPayload = JSON.parse(JSON.stringify(parsedJson));
  clonedPayload.model = model;
  const serialized = JSON.stringify(clonedPayload);
  return Buffer.from(serialized, 'utf-8');
}

async function buildCodeAssistPayload(body) {
  const payload = isPlainObject(body) ? JSON.parse(JSON.stringify(body)) : {};
  const validation = validateCodeAssistRequest(payload);
  if (validation.error) {
    return { error: validation.error };
  }

  const mode = normalizeCodeAssistMode(validation.mode);
  const task = normalizeTextField(payload.task ?? payload.prompt ?? payload.request ?? '');
  const error = normalizeTextField(payload.error ?? payload.exception ?? '');
  const currentCode = normalizeTextField(payload.currentCode ?? payload.code ?? payload.source ?? '');
  const constraints = normalizeListField(payload.constraints ?? payload.requirements ?? []);
  const outputFormat = normalizeTextField(payload.expectedOutput ?? payload.outputFormat ?? 'Return a clean patch or code change.');
  const queryOverride = normalizeTextField(payload.queryOverride);
  const contextQuery = queryOverride || normalizeTextField(
    payload.contextQuery ??
      [task, error, currentCode].filter(Boolean).join('\n')
  );
  const topK = parsePositiveInt(payload.topK, 6, 1, 20);
  const maxContextChars = parsePositiveInt(payload.maxContextChars, 12000, 1000, 100000);
  const previewOnly = Boolean(payload.previewOnly ?? payload.dryRun ?? payload.promptPreview);
  if (!task && !currentCode && !error) {
    return { error: 'task, code, or error is required' };
  }

  const relatedContext = await fetchRagContext({
    query: contextQuery,
    topK,
    maxContextChars,
    sourceKinds: normalizeCsvList(payload.sourceKinds ?? RAG_SOURCE_KINDS)
  });

  const prompt = buildStructuredCodePrompt({
    role: normalizeTextField(payload.role ?? 'You are a careful coding assistant.'),
    mode,
    task,
    error,
    currentCode,
    relatedContext,
    constraints,
    expectedOutput: outputFormat,
    maxContextChars
  });

  const messages = buildCodeAssistMessages(payload.messages, prompt, mode);

  payload.messages = messages;
  payload.stream = payload.stream ?? true;
  if (!payload.model) {
    payload.model = getDefaultFreeModel();
  }
  return {
    mode,
    previewOnly,
    query: contextQuery,
    retrieval: {
      topK,
      maxContextChars,
      sourceKinds: normalizeCsvList(payload.sourceKinds ?? RAG_SOURCE_KINDS),
      matches: relatedContext
    },
    prompt,
    messages,
    model: payload.model,
    parsedJson: payload,
    body: Buffer.from(JSON.stringify(payload), 'utf-8')
  };
}

async function fetchRagContext({ query, topK, maxContextChars, sourceKinds }) {
  const cleanedQuery = normalizeTextField(query);
  if (!cleanedQuery) {
    return [];
  }

  const response = await fetch(`${RAG_MCP_URL.replace(/\/+$/, '')}/api/query`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      query: cleanedQuery,
      topK,
      sourceKinds: sourceKinds.length ? sourceKinds : undefined
    })
  });

  const payload = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new Error(typeof payload.error === 'string' ? payload.error : `RAG query failed with HTTP ${response.status}`);
  }

  const matches = Array.isArray(payload.matches) ? payload.matches : [];
  return trimMatchesToContext(matches, maxContextChars);
}

function buildStructuredCodePrompt({
  role,
  mode,
  task,
  error,
  currentCode,
  relatedContext,
  constraints,
  expectedOutput,
  maxContextChars
}) {
  const contextBlock = relatedContext.length
    ? relatedContext.map((match, index) => {
        const locator = [match.label, match.locator].filter(Boolean).join(' | ');
        return `${index + 1}. score=${formatScore(match.score)} source=${locator}\n${match.snippet}`;
      }).join('\n\n')
    : 'No relevant context found.';

  const modeBlock = buildModeInstruction(mode);

  return [
    'ROLE',
    role,
    '',
    'MODE',
    mode,
    '',
    'INSTRUCTION',
    modeBlock,
    '',
    'TASK',
    task || 'No task provided.',
    '',
    'ERROR',
    error || 'No error provided.',
    '',
    'CURRENT CODE',
    currentCode || 'No code provided.',
    '',
    'RELATED CONTEXT',
    contextBlock,
    '',
    'CONSTRAINTS',
    constraints.length ? constraints.map((item) => `- ${item}`).join('\n') : '- Preserve existing behavior.',
    '',
    'EXPECTED OUTPUT',
    expectedOutput,
    '',
    'CONTEXT LIMIT',
    String(maxContextChars)
  ].join('\n');
}

function normalizeCodeAssistMode(mode) {
  const value = normalizeTextField(mode).toLowerCase();
  if (['explain', 'modify', 'fix', 'debug', 'repair'].includes(value)) {
    return value;
  }
  return 'modify';
}

function buildModeInstruction(mode) {
  switch (mode) {
    case 'explain':
      return 'Explain what the code does, identify important control flow, and call out likely failure points. Do not propose edits unless the prompt explicitly asks for them.';
    case 'modify':
      return 'Make the requested code change. Prefer a minimal patch, preserve existing behavior, and explain any non-obvious tradeoffs briefly.';
    case 'fix':
      return 'Fix the specific bug or failure described by the error. Focus on the smallest safe correction and include the exact code change needed.';
    case 'debug':
      return 'Diagnose the failure step by step, separate symptoms from root cause, and provide the most likely fix with supporting reasoning.';
    case 'repair':
      return 'Repair the code so it is correct, robust, and consistent with the surrounding implementation. If a direct fix is uncertain, state the safest fallback.';
    default:
      return 'Make the requested code change with minimal disruption.';
  }
}

function normalizeTextField(value) {
  return typeof value === 'string' ? value.trim() : '';
}

function normalizeListField(value) {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.map((item) => normalizeTextField(item)).filter(Boolean);
}

function normalizeCsvList(value) {
  if (Array.isArray(value)) {
    return value.map((item) => normalizeTextField(item)).filter(Boolean);
  }
  return normalizeTextField(value)
    ? normalizeTextField(value).split(',').map((item) => item.trim()).filter(Boolean)
    : [];
}

function parseJsonBody(body) {
  if (!body || !body.length) {
    return { value: {} };
  }

  try {
    const value = JSON.parse(body.toString('utf-8'));
    if (!isPlainObject(value)) {
      return { error: 'request body must be a JSON object' };
    }
    return { value };
  } catch (error) {
    return { error: `invalid JSON body: ${error.message}` };
  }
}

function validateCodeAssistRequest(payload) {
  const mode = normalizeTextField(payload.mode ?? 'modify').toLowerCase();
  if (!['explain', 'modify', 'fix', 'debug', 'repair'].includes(mode)) {
    return { error: `mode must be one of: explain, modify, fix, debug, repair` };
  }

  if (payload.topK !== undefined && parsePositiveInt(payload.topK, null, 1, 20) === null) {
    return { error: 'topK must be an integer between 1 and 20' };
  }

  if (payload.maxContextChars !== undefined && parsePositiveInt(payload.maxContextChars, null, 1000, 100000) === null) {
    return { error: 'maxContextChars must be an integer between 1000 and 100000' };
  }

  return { mode };
}

function buildCodeAssistMessages(existingMessages, prompt, mode) {
  const modeSystem = buildModeSystemMessage(mode);
  if (Array.isArray(existingMessages) && existingMessages.length) {
    return [
      { role: 'system', content: modeSystem },
      ...existingMessages,
      { role: 'user', content: prompt }
    ];
  }

  return [
    { role: 'system', content: modeSystem },
    { role: 'user', content: prompt }
  ];
}

function buildModeSystemMessage(mode) {
  switch (mode) {
    case 'explain':
      return 'You are in explain mode. Prioritize clarity, structure, and root-cause understanding over edits.';
    case 'modify':
      return 'You are in modify mode. Produce a minimal, reviewable code change.';
    case 'fix':
      return 'You are in fix mode. Focus on the exact bug and the smallest safe correction.';
    case 'debug':
      return 'You are in debug mode. Analyze the failure carefully and isolate the cause before suggesting changes.';
    case 'repair':
      return 'You are in repair mode. Restore correctness while preserving surrounding behavior.';
    default:
      return 'You are a careful coding assistant.';
  }
}

function parsePositiveInt(value, fallback, min, max) {
  if (value === undefined || value === null || value === '') {
    return fallback;
  }
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < min || parsed > max) {
    return null;
  }
  return parsed;
}

function trimMatchesToContext(matches, maxContextChars) {
  if (!Array.isArray(matches) || !matches.length) {
    return [];
  }

  const limit = Number.isInteger(maxContextChars) && maxContextChars > 0 ? maxContextChars : 12000;
  let total = 0;
  const trimmed = [];

  for (const match of matches) {
    const snippet = normalizeTextField(match?.snippet);
    const remaining = limit - total;
    if (remaining <= 0) {
      break;
    }
    const nextSnippet = snippet.length > remaining ? snippet.slice(0, remaining) : snippet;
    total += nextSnippet.length;
    trimmed.push({ ...match, snippet: nextSnippet });
  }

  return trimmed;
}

function formatScore(score) {
  return Number.isFinite(Number(score)) ? Number(score).toFixed(3) : '0.000';
}

function isPlainObject(value) {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function shouldRetryStatus(status) {
  return status >= 500 || status === 402 || status === 404 || status === 408 || status === 429;
}
