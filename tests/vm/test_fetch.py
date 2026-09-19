import base64
import hashlib
import io
import tarfile

import httpx
import pytest

from cortex.rlm.models import VmContext
from cortex.vm.fetch import fetch_artifact
from cortex.vm.guest import GuestIdentity
from cortex.vm.models import VmError


def archive():
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w") as stream:
        member = tarfile.TarInfo("candidate.py")
        member.size = 5
        stream.addfile(member, io.BytesIO(b"pass\n"))
    return output.getvalue()


def binding():
    return GuestIdentity("vm-one", "topic-one", "ab" * 32, "topic"), VmContext(
        topic_id="topic-one",
        job_id="job-one",
        image_digest="ab" * 32,
        purpose="evaluate",
        artifact_digest=hashlib.sha256(archive()).hexdigest(),
    )


async def test_topic_guest_returns_exact_served_bytes_without_retarring():
    identity, context = binding()
    result = await fetch_artifact(
        identity,
        context,
        "https://artifact.example/candidate.tar",
        transport=httpx.MockTransport(
            lambda request: httpx.Response(200, stream=httpx.ByteStream(archive()))
        ),
    )
    assert base64.b64decode(result) == archive()


@pytest.mark.parametrize("failure", ["digest", "redirect", "empty", "wrong-topic", "sister"])
async def test_fetch_refuses_misbound_or_unverifiable_artifacts(failure):
    identity, context = binding()
    if failure == "wrong-topic":
        context = context.model_copy(update={"topic_id": "other-topic"})
    elif failure == "sister":
        identity = GuestIdentity("vm-one", "topic-one", "ab" * 32, "experiment")
    elif failure == "digest":
        context = context.model_copy(update={"artifact_digest": "cd" * 32})
    status = 302 if failure == "redirect" else 200
    data = b"" if failure == "empty" else archive()
    with pytest.raises(VmError):
        await fetch_artifact(
            identity,
            context,
            "https://artifact.example/candidate.tar",
            transport=httpx.MockTransport(
                lambda request: httpx.Response(
                    status, stream=httpx.ByteStream(data), headers={"location": "https://elsewhere"}
                )
            ),
        )
