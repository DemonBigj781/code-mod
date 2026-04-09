import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { once } from 'node:events';
import test from 'node:test';
import { RagIndex } from '../src/rag.js';
class FakeEmbeddingProvider {
    name = 'fake-hashed-bow';
    async embed(texts) {
        return texts.map((text) => this.embedOne(text));
    }
    embedOne(text) {
        const dimensions = 128;
        const vector = new Array(dimensions).fill(0);
        const tokens = text
            .toLowerCase()
            .split(/[^a-z0-9_]+/g)
            .filter(Boolean);
        for (const token of tokens) {
            const index = hashToken(token) % dimensions;
            vector[index] += 1;
        }
        const magnitude = Math.sqrt(vector.reduce((sum, value) => sum + value * value, 0));
        return magnitude ? vector.map((value) => value / magnitude) : vector;
    }
}
test('ingests files, urls, and inline source code, then retrieves ranked snippets', async () => {
    const tempDir = await mkdtemp(path.join(os.tmpdir(), 'rag-mcp-server-'));
    const filePath = path.join(tempDir, 'notes.md');
    const codePath = path.join(tempDir, 'helpers.ts');
    await writeFile(filePath, [
        '# Mountain Fortress',
        '',
        'A mountain fortress with a central lake and hot springs.',
        '',
        'This document should be easy to retrieve.'
    ].join('\n'), 'utf8');
    await writeFile(codePath, [
        'export function buildGreeting(name: string): string {',
        '  return `Hello, ${name}!`;',
        '}',
        '',
        'export const greetingStyle = "warm";'
    ].join('\n'), 'utf8');
    const remoteServer = createServer((_req, res) => {
        res.writeHead(200, { 'content-type': 'text/plain; charset=utf-8' });
        res.end('A river delta with fertile soil and a quiet harbor.');
    });
    remoteServer.listen(0);
    await once(remoteServer, 'listening');
    const address = remoteServer.address();
    assert.ok(address && typeof address === 'object');
    const url = `http://127.0.0.1:${address.port}/doc`;
    const index = new RagIndex({
        dataDir: path.join(tempDir, 'data'),
        embeddingProvider: new FakeEmbeddingProvider()
    });
    const sources = [
        { kind: 'file', locator: filePath, path: filePath, label: 'Notes' },
        { kind: 'code', locator: codePath, path: codePath, label: 'Helpers' },
        { kind: 'url', locator: url, url, label: 'Remote Doc' },
        {
            kind: 'text',
            locator: 'inline://snippet',
            label: 'Inline Snippet',
            content: 'Source code can also be supplied inline for future connectors.'
        }
    ];
    const ingest = await index.ingestSources(sources);
    assert.equal(ingest.length, 4);
    const sourcesList = await index.listSources();
    assert.equal(sourcesList.length, 4);
    const codeSource = ingest.find((item) => item.label === 'Helpers');
    assert.ok(codeSource);
    const chunks = await index.listChunks(codeSource.sourceId);
    assert.ok(chunks.length >= 1);
    assert.match(chunks[0].text, /buildGreeting/);
    const queryResults = await index.search({ query: 'buildGreeting', topK: 3 });
    assert.ok(queryResults.length >= 1);
    assert.equal(queryResults[0].sourceId, codeSource.sourceId);
    assert.match(queryResults[0].snippet, /buildGreeting/);
    const semanticResults = await index.search({ query: 'hot springs fortress', topK: 3 });
    assert.ok(semanticResults.length >= 1);
    assert.match(semanticResults[0].snippet, /Mountain Fortress|hot springs/i);
    const reloaded = new RagIndex({
        dataDir: path.join(tempDir, 'data'),
        embeddingProvider: new FakeEmbeddingProvider()
    });
    const persistedSources = await reloaded.listSources();
    assert.equal(persistedSources.length, 4);
    const persistedResults = await reloaded.search({ query: 'river delta', topK: 1 });
    assert.equal(persistedResults[0].label, 'Remote Doc');
    await remoteServer.close();
    await rm(tempDir, { recursive: true, force: true });
});
test('persists the index file on disk', async () => {
    const tempDir = await mkdtemp(path.join(os.tmpdir(), 'rag-mcp-server-persist-'));
    const index = new RagIndex({
        dataDir: path.join(tempDir, 'data'),
        embeddingProvider: new FakeEmbeddingProvider()
    });
    await index.ingestSources([
        {
            kind: 'text',
            locator: 'inline://persist',
            label: 'Persisted Doc',
            content: 'Persistence matters for later documentation and citations.'
        }
    ]);
    const indexPath = path.join(tempDir, 'data', 'index.json');
    const raw = await readFile(indexPath, 'utf8');
    const parsed = JSON.parse(raw);
    assert.equal(parsed.sources && Object.keys(parsed.sources).length, 1);
    assert.equal(parsed.chunks && Object.keys(parsed.chunks).length >= 1, true);
    await rm(tempDir, { recursive: true, force: true });
});
function hashToken(token) {
    let hash = 0;
    for (let index = 0; index < token.length; index += 1) {
        hash = (hash * 31 + token.charCodeAt(index)) >>> 0;
    }
    return hash;
}
//# sourceMappingURL=rag.test.js.map