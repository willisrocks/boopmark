import { describe, it, expect, vi, beforeEach } from 'vitest';
import upload from './upload';

describe('upload', () => {
  const mockFetch = vi.fn();

  beforeEach(() => {
    vi.stubGlobal('fetch', mockFetch);
    mockFetch.mockReset();
  });

  it('sends JSON with url when url is provided', async () => {
    mockFetch.mockResolvedValue({
      json: () => Promise.resolve({ url: 'https://cdn.example.com/file.png', mimeType: 'image/png' }),
    });

    const result = await upload({ url: 'https://example.com/image.png' });

    expect(mockFetch).toHaveBeenCalledWith('https://create.xyz/api/v0/upload', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ url: 'https://example.com/image.png' }),
    });
    expect(result).toEqual({ url: 'https://cdn.example.com/file.png', mimeType: 'image/png' });
  });

  it('sends JSON with base64 when base64 is provided', async () => {
    mockFetch.mockResolvedValue({
      json: () => Promise.resolve({ url: 'https://cdn.example.com/file.png', mimeType: 'image/png' }),
    });

    await upload({ base64: 'abc123==' });

    const [, init] = mockFetch.mock.calls[0];
    expect(init.headers['Content-Type']).toBe('application/json');
    expect(JSON.parse(init.body)).toEqual({ base64: 'abc123==' });
  });

  it('sends buffer with octet-stream content type when buffer is provided', async () => {
    const buf = new Uint8Array([1, 2, 3]);
    mockFetch.mockResolvedValue({
      json: () => Promise.resolve({ url: 'https://cdn.example.com/file.bin', mimeType: null }),
    });

    await upload({ buffer: buf });

    const [, init] = mockFetch.mock.calls[0];
    expect(init.headers['Content-Type']).toBe('application/octet-stream');
    expect(init.body).toBe(buf);
  });

  it('returns null mimeType when not provided in response', async () => {
    mockFetch.mockResolvedValue({
      json: () => Promise.resolve({ url: 'https://cdn.example.com/file.bin' }),
    });

    const result = await upload({ url: 'https://example.com/file' });

    expect(result.mimeType).toBeNull();
  });
});
