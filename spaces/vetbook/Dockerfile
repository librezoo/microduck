# Bookworm, pinned: Playwright's `--with-deps` knows Debian 12, and on the unpinned `-slim`, which
# is Debian 13 now, it falls back to an Ubuntu 20.04 package list that names fonts trixie no
# longer has (`ttf-unifont`, `ttf-ubuntu-font-family`) and the build stops there.
FROM python:3.12-slim-bookworm

# `git` because Hugging Face appends a `git config --global` layer to every Docker Space, and a
# `-slim` base has none — that layer then fails with exit 127 after the image has built fine.
RUN apt-get update \
 && apt-get install -y --no-install-recommends git \
 && rm -rf /var/lib/apt/lists/*

# Playwright and its Chromium, installed as root because `--with-deps` installs system libraries.
# The browser goes somewhere shared, so the uid-1000 user below can run it.
ENV PLAYWRIGHT_BROWSERS_PATH=/ms-playwright
COPY requirements.txt /tmp/requirements.txt
RUN pip install --no-cache-dir -r /tmp/requirements.txt \
 && playwright install --with-deps chromium \
 && chmod -R a+rX /ms-playwright

# The canonical uid-1000 `user` at `/home/user`: the platform's appended layers assume both.
RUN useradd -m -u 1000 user
USER user
ENV HOME=/home/user \
    PYTHONUNBUFFERED=1
WORKDIR $HOME/app
COPY --chown=user . $HOME/app

EXPOSE 7860
CMD ["uvicorn", "app:app", "--host", "0.0.0.0", "--port", "7860"]
