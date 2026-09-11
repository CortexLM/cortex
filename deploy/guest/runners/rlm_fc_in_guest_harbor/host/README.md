# Host RCA helpers

Not on the guest `run` path. Copy onto a retained jail on the KVM host.

`summarize_job.py` unwraps orchestrator `job.out` (`RunJobResponse.output` →
adjacent-tagged `VmJobOutput`, including `body.report` on evaluated jobs)
so `primary_value=` / `n_measured=` / `cv=` print for **any** job directory.
There is no baked `JOBDIR` (the metal n15 copy hardcoded
`/var/lib/proof/pathc-baseline-n15/...`).

```bash
python3 summarize_job.py --jobdir /path/to/retained/jail
python3 summarize_job.py /path/to/job.out
```

Incomplete `harvest-work` vs the guest overlay is a host harvest refuse
(`proof-fc-harvest`), not something this script invents a score for.
Guest already writes `verifier/reward.txt`; do not "fix" a lagging dump by
always-writing that file from the host.
