# Host RCA helpers

Not on the guest `run` path. Copy onto a retained jail on the KVM host.

`summarize_job.py` unwraps orchestrator `job.out` (`RunJobResponse.output` →
adjacent-tagged `VmJobOutput`, including `body.report` on evaluated jobs)
so `primary_value=` / `n_measured=` / `cv=` print for **any** job directory.
It then writes `custom_value.txt` and `summary.txt` under JOBDIR. JOBDIR or
a `job.out` path is required — no stdin (a missing arg is fail-closed, not a
hang). There is no baked `JOBDIR` and no Harbor job id (`n15-…`). Discovery
walks for `job.out` and prefers the newest file that unwraps a finite
`primary_value`, so a stale metal job name cannot empty `cv=`.

`run-n15` always waits for Harbor `curl.pid` / `job.out` and always invokes
`summarize_job.py`. `--restart` still logs `job_http=` and `DONE` (metal
shortpack a21f stopped at `N15 RESTART` with curl orphaned and no summary).

```bash
python3 summarize_job.py --jobdir /path/to/retained/jail
python3 summarize_job.py /path/to/job.out
./run-n15 --restart /path/to/jobdir
```

Incomplete `harvest-work` vs the guest overlay is a host harvest refuse
(`proof-fc-harvest`), not something this script invents a score for.
A vsock `Done` still refreshes `harvest-work` until the dump would pass
those checks (debugfs race: `reward.txt` can land ~12s before `result.json`).
Guest already writes `verifier/reward.txt`; do not "fix" a lagging dump by
always-writing that file from the host.
