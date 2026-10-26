from hypertrain.gpu_ops.launcher import direct_ssh


def test_direct_ssh_uses_public_mapping_of_port_22():
    st = {
        "public_ipaddr": "47.186.21.5 ",
        "ports": {"22/tcp": [{"HostIp": "0.0.0.0", "HostPort": "52555"}]},
    }
    assert direct_ssh(st) == ("47.186.21.5", 52555)


def test_direct_ssh_absent_or_malformed_falls_back_to_proxy():
    assert direct_ssh({"public_ipaddr": "1.2.3.4", "ports": None}) is None
    assert (
        direct_ssh({"public_ipaddr": "1.2.3.4", "ports": {"22/tcp": [{"HostPort": "x;rm"}]}})
        is None
    )
    assert direct_ssh({"ports": {"22/tcp": [{"HostPort": "22"}]}}) is None
