const originalFetch = fetch;
const isBackend = () => typeof window === 'undefined';

const safeStringify = (value: unknown) =>
  JSON.stringify(value, (_k, v) => {
    if (v instanceof Date) return { __t: 'Date', v: v.toISOString() };
    if (v instanceof Error)
      return { __t: 'Error', v: { name: v.name, message: v.message, stack: v.stack } };
    return v;
  });

const postToParent = (level: string, text: string, extra: unknown) => {
  try {
    if (isBackend() || !window.parent || window.parent === window) {
      ('level' in console ? console[level] : console.log)(text, extra);
      return;
    }
    window.parent.postMessage(
      {
        type: 'sandbox:web:console-write',
        __viteConsole: true,
        level,
        text,
        args: [safeStringify(extra)],
      },
      '*'
    );
  } catch {
    /* noop */
  }
};

const getUrlFromArgs = (...args: Parameters<typeof originalFetch>): string => {
  const [input] = args;
  if (typeof input === 'string') return input;
  if (input instanceof Request) return input.url;
  return `${input.protocol}//${input.host}${input.pathname}`;
};

const isFirstPartyURL = (url: string) => {
  return url.startsWith('/integrations') || url.startsWith('/_create');
};

const isSecondPartyUrl = (url: string) => {
  return (
    (process.env.NEXT_PUBLIC_CREATE_API_BASE_URL &&
      url.startsWith(process.env.NEXT_PUBLIC_CREATE_API_BASE_URL)) ||
    (process.env.NEXT_PUBLIC_CREATE_BASE_URL &&
      url.startsWith(process.env.NEXT_PUBLIC_CREATE_BASE_URL)) ||
    url.startsWith('https://www.create.xyz') ||
    url.startsWith('https://api.create.xyz/') ||
    url.startsWith('https://www.createanything.com') ||
    url.startsWith('https://api.createanything.com')
  );
};

export const fetchWithHeaders = async (
  input: RequestInfo | URL,
  init?: RequestInit
): Promise<Response> => {
  const url = getUrlFromArgs(input, init);

  const additionalHeaders = {
    'x-createxyz-project-group-id': process.env.NEXT_PUBLIC_PROJECT_GROUP_ID,
  };

  const isExternalFetch = !isFirstPartyURL(url) && !isSecondPartyUrl(url);
  // we should not add headers to requests that don't go to our own server
  // or if it's an API request
  if (isExternalFetch || url.startsWith('/api')) {
    return originalFetch(input, init);
  }

  // Normalize Request into init
  let finalInit: RequestInit;
  if (input instanceof Request) {
    const hasBody = !!input.body;
    finalInit = {
      method: input.method,
      headers: new Headers(input.headers),
      body: input.body,
      mode: input.mode,
      credentials: input.credentials,
      cache: input.cache,
      redirect: input.redirect,
      referrer: input.referrer,
      referrerPolicy: input.referrerPolicy,
      integrity: input.integrity,
      keepalive: input.keepalive,
      signal: input.signal,
      ...(hasBody ? { duplex: 'half' } : {}),
      ...init,
    };
  } else {
    finalInit = { ...init, headers: new Headers(init?.headers ?? {}) };
  }

  const finalHeaders = new Headers(finalInit.headers);
  for (const [key, value] of Object.entries(additionalHeaders)) {
    if (value) finalHeaders.set(key, value);
  }
  finalInit.headers = finalHeaders;

  const prefix = !isSecondPartyUrl(url)
    ? isBackend()
      ? (process.env.NEXT_PUBLIC_CREATE_BASE_URL ?? 'https://www.create.xyz')
      : ''
    : '';

  try {
    const result = await originalFetch(`${prefix}${url}`, finalInit);
    if (!result.ok) {
      postToParent(
        'error',
        `Failed to load resource: the server responded with a status of ${result.status} (${result.statusText ?? ''})`,
        {
          url,
          status: result.status,
          statusText: result.statusText,
        }
      );
    }
    return result;
  } catch (error) {
    postToParent('error', 'Fetch error', {
      url,
      error:
        error instanceof Error
          ? { name: error.name, message: error.message, stack: error.stack }
          : error,
    });
    throw error;
  }
};

export default fetchWithHeaders;
