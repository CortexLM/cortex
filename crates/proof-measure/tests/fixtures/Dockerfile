# TEST-ONLY observer image. This is NOT the real proof-eval scoring image: it
# implements only the `proof-eval score --request … --out …` CLI contract with
# deterministic numbers derived from the request so the controller-side
# DockerObserver plumbing can be exercised without a GPU, a judge, or a model.
FROM python:3.12-slim
LABEL cortex.test.observer="1"
COPY proof-eval.py /usr/bin/proof-eval
RUN chmod 0555 /usr/bin/proof-eval
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/python", "-I", "/usr/bin/proof-eval"]
