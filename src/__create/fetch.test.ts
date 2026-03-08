import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock fetch before module loads (fetchWithHeaders captures fetch at import time)
const mockFetch = vi.fn();
vi.stubGlobal('fetch', mockFetch);

const { fetchWithHeaders } = await import('./fetch');

describe('fetchWithHeaders', () => {
  beforeEach(() => {
    mockFetch.mockReset();
    mockFetch.mockResolvedValue(new Response('ok', { status: 200 }));
  });

  it('passes external URLs through without extra headers', async () => {
    await fetchWithHeaders('https://example.com/data');

    expect(mockFetch).toHaveBeenCalledWith('https://example.com/data', undefined);
  });

  it('passes /api URLs through without extra headers', async () => {
    await fetchWithHeaders('http://localhost/api/users');

    expect(mockFetch).toHaveBeenCalledWith('http://localhost/api/users', undefined);
  });

  it('adds headers for first-party /integrations URLs', async () => {
    await fetchWithHeaders('/integrations/foo');

    const [, init] = mockFetch.mock.calls[0];
    expect(init.headers).toBeInstanceOf(Headers);
  });

  it('adds headers for first-party /_create URLs', async () => {
    await fetchWithHeaders('/_create/something');

    const [, init] = mockFetch.mock.calls[0];
    expect(init.headers).toBeInstanceOf(Headers);
  });

  it('adds headers for create.xyz URLs', async () => {
    await fetchWithHeaders('https://www.create.xyz/foo');

    const [, init] = mockFetch.mock.calls[0];
    expect(init.headers).toBeInstanceOf(Headers);
  });

  it('throws when fetch fails', async () => {
    mockFetch.mockRejectedValue(new Error('network error'));

    await expect(fetchWithHeaders('https://example.com/x')).rejects.toThrow('network error');
  });
});
