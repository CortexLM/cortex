# syntax=docker/dockerfile:1.7
# Relearn eval image. Cortex pins the digest, never a floating tag.
ARG PYTHON_IMAGE=python@sha256:519591d6871b7bc437060736b9f7456b8731f1499a57e22e6c285135ae657bf7
FROM ${PYTHON_IMAGE}
WORKDIR /eval
COPY harness /eval/harness
COPY generators /eval/generators
COPY teacher /eval/teacher
COPY decontam /eval/decontam
ENV PYTHONPATH=/eval
ENTRYPOINT ["python", "-m", "harness.eval"]
