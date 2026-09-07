FROM python@sha256:78387bc3881b8273120a12ebe6c1ab22b018ccc2c9adf565ae1ac9b536e184ea
COPY prime-agent-runtime/uv.lock /opt/atlas/uv.lock
COPY sandbox/locked_requirements.py /opt/atlas/locked_requirements.py
RUN python /opt/atlas/locked_requirements.py /opt/atlas/uv.lock > /opt/atlas/requirements.txt \
    && pip install --no-cache-dir --only-binary=:all: --require-hashes -r /opt/atlas/requirements.txt
COPY prime-agent-runtime/src/rlm /opt/atlas/rlm
ENV PYTHONPATH=/opt/atlas PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
USER 65532:65532
WORKDIR /work
ENTRYPOINT ["/usr/local/bin/python", "-m", "rlm.repl"]
