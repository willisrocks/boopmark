(() => {
  if (!import.meta.env.DEV) return; // skip in prod

  let postCount = 0;
  let lastPostTime = 0;
  let lastErrorKey = null;
  const MAX_ERROR_POSTS = 5;
  const THROTTLE_MS = 1000;

  function sendError(msg, stack) {
    const errorKey = `${msg}::${stack}`;

    if (errorKey !== lastErrorKey) {
      lastErrorKey = errorKey;
      postCount = 0;
    }

    if (postCount >= MAX_ERROR_POSTS) {
      return;
    }

    const now = Date.now();
    const timeSinceLastPost = now - lastPostTime;

    const post = () => {
      if (postCount >= MAX_ERROR_POSTS) {
        return;
      }
      postCount += 1;
      lastPostTime = Date.now();
      window.parent?.postMessage(
        {
          type: 'sandbox:error:detected',
          error: { message: msg, stack: stack, name: 'Error' },
        },
        '*'
      );
    };

    if (timeSinceLastPost < THROTTLE_MS) {
      setTimeout(post, THROTTLE_MS - timeSinceLastPost);
    } else {
      post();
    }

    console.error(msg, stack);
  }

  /* vite HMR channel ------------------------------------------------- */
  if (import.meta.hot) {
    import.meta.hot.on('vite:error', (payload) => {
      const err = payload.err || payload;
      sendError(err.message, err.stack);
    });
  }
})();
