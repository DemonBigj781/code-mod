import { request } from 'playwright';

const api = await request.newContext({
  baseURL: 'http://localhost:8080'
});

try {
  const response = await api.get('/models');
  const payload = await response.json();

  console.log('uiModels:', (payload.uiModels ?? []).join(', '));

  if ((payload.uiModels ?? []).includes('openrouter/free')) {
    throw new Error('openrouter/free should not be in uiModels');
  }
} finally {
  await api.dispose();
}
