# Provenance: import of CortexLM/hypertrain into CortexLM/cortex

This record shows that branch `v3` of `CortexLM/cortex` contains the full history of
`CortexLM/hypertrain` `main` under `hypertrain/`, with every imported file byte-identical.
Decision and layout: [ADR 0001](../adr/0001-monorepo-structure.md). Every command below
can be rerun from public clones; results are those observed on 2026-10-26.

## Source and destination

| | Value |
| --- | --- |
| Source | `https://github.com/CortexLM/hypertrain`, branch `main` only |
| Source tip | `679eb5b3b2665c9a81a54fc3f7332e75bbe53394` (2026-10-25, = `git ls-remote` `refs/heads/main` on 2026-10-26), tree `371acfcc4d08942a85a824d9b048c9d8ac24c495` |
| Source root | `03de4fb23806a6342969f5ebb2cd224d1c40a3af` ("Hypertrain v0.1.0", 2026-10-08) |
| Source size | 236 commits (29 merges), 478 files, 7,764,827 bytes, no tag; 31 other branches not imported |
| Cortex base | `main` `008610a142f62c113faede035ad5b3706a977ddd`: 973 commits, 251 files, 6,211,217 bytes |
| Rewritten tip | `18ae364df450c062a72b40f100ed3c07657fcadd`; `18ae364d:hypertrain` = tree `371acfcc…` |
| Rewritten root | `728e693e1f43b83fdb96bf44055809c660ba481d` |
| Merge commit | `e61461c39f8092fc42ade5f0fa5852e6ea8491a5`: first parent `63f08dcd` (Cortex, the build-check commit on top of `008610a1`), second parent `18ae364d` |
| After the merge | 1,211 commits (973 + 236 + 2), 729 files (251 + 478), 13,976,695 bytes (+651 from `63f08dcd`), two root commits (`2a16f5bc`, `728e693e`) |

Nothing on Cortex `main` was rewritten. No tag was created or imported.

## Rewrite

In a throwaway clone of hypertrain `main` (git 2.53.0, git-filter-repo 2.47.0 in a private
virtual environment):

```sh
git filter-repo --to-subdirectory-filter hypertrain \
  --replace-message <(printf '%s\n' 'regex:(?<![\w#&])#([0-9]+)\b==>CortexLM/hypertrain#\1')
```

- Every path moves under `hypertrain/`. File contents, authors, committers and dates are
  unchanged: the concatenated author/committer names, e-mails and dates of all 236 commits
  hash to the same value before and after.
- The 42 issue and PR references (`#N`) in the messages now read `CortexLM/hypertrain#N`,
  so they keep pointing at hypertrain and can never close a Cortex issue. No bare `#N`
  remains.
- filter-repo rewrote abbreviated commit hashes in messages to the new hashes. It left 21
  strings that look like hashes but are not commits of `main` as-is: numbers, run IDs and
  commits from elsewhere.

The rewritten `main` was merged into the Cortex branch with
`git merge --allow-unrelated-histories` (commit `e61461c3`).

## Byte identity

Each of the 236 rewritten commits has a `hypertrain/` tree equal to the tree of its
original commit, so every historical version of every file is byte-identical. Check with
both clones side by side, using the table below:

```sh
grep -E '^\| [0-9]+ \|' docs/provenance/hypertrain-import.md \
  | awk -F'|' '{gsub(/[ `]/, "", $3); gsub(/[ `]/, "", $4); print $3, $4}' \
  | while read -r old new; do
      test "$(git -C ../hypertrain rev-parse "$old^{tree}")" = "$(git rev-parse "$new:hypertrain")" \
        || echo "MISMATCH $old $new"
    done
```

Observed: no mismatch, 236 of 236.

Rename-only proof, in a scratch repository that fetched both tips:

```sh
git diff --find-renames=100% --name-status 679eb5b3 18ae364d | cut -f1 | sort | uniq -c
#     478 R100
git diff --find-renames=100% --stat 679eb5b3 18ae364d | tail -1
#  478 files changed, 0 insertions(+), 0 deletions(-)
```

Each `R100` pairs `X` with `hypertrain/X`. This covers the files whose bytes carry a
verifiable property:

| Property | Files (under `hypertrain/`) |
| --- | --- |
| Golden leaf roots | `tests/trainer/test_golden_replay.py`, `tests/protocol/test_od_fields.py`, `scripts/check-proof.sh` |
| `implementation_hash` inputs | `src/hypertrain/challenge/store.py` and the nine files listed at its line 4249 |
| Protocol constants | `src/hypertrain/protocol/*.py` |

## History is preserved: `git log --follow`

The per-file history in the monorepo maps commit by commit, through the table below, onto
the history in the original repository:

```sh
git -C ../hypertrain log --follow --format=%H 679eb5b3 -- src/hypertrain/challenge/store.py
git log --follow --format=%H e61461c3 -- hypertrain/src/hypertrain/challenge/store.py
```

| File | Commits (original / monorepo) | Mapped lists equal | Oldest commit (original → monorepo) |
| --- | --- | --- | --- |
| `src/hypertrain/challenge/store.py` | 26 / 26 | yes | `03de4fb2` → `728e693e` |
| `tests/trainer/test_golden_replay.py` | 4 / 4 | yes | `05a1d352` → `a551034e` |
| `src/hypertrain/protocol/messages_v2.py` | 5 / 5 | yes | `8c072448` → `a7489c3c` |

## Issues, pull requests, branches and attestations

- Issues and PRs stay in `CortexLM/hypertrain` (open on 2026-10-26: #25, #28, #29, #32, #34);
  the imported messages link to them.
- Unmerged branches are not imported. One moves later with the same filter-repo command,
  or as patches applied with `git am --directory=hypertrain`.
- Attestations of images built before the import name `CortexLM/hypertrain` and stay valid
  there. Images built from the monorepo carry the `CortexLM/cortex` identity (ADR 0001 §6).

## After the import

The later `v3` commits change nothing under `hypertrain/` except its `.github/` directory.
GitHub only reads the root `.github/workflows/`, so the workflows moved there (ADR 0001 §5):

```sh
git diff --stat e61461c3 v3 -- hypertrain ':!hypertrain/.github'     # empty
git diff -M --name-status e61461c3 v3 -- hypertrain/.github .github/workflows/hypertrain-*
# R077  hypertrain/.github/workflows/ci.yml          .github/workflows/hypertrain-ci.yml
# R095  hypertrain/.github/workflows/heavy.yml       .github/workflows/hypertrain-heavy.yml
# R066  hypertrain/.github/workflows/miner-flow.yml  .github/workflows/hypertrain-miner-flow.yml
# A     .github/workflows/hypertrain-images.yml      (rewritten from images.yml)
# D     hypertrain/.github/workflows/images.yml
# D     hypertrain/.github/workflows/gpu-vast.yml     (not ported)
# D     hypertrain/.github/workflows/release.yml      (not ported)
```

The moved workflows keep their history. `git log --follow --
.github/workflows/hypertrain-ci.yml` lists the move commit, then the 13 commits of the
original `ci.yml` down to `728e693e`. `hypertrain-heavy.yml` shows 13 + 1 and
`hypertrain-miner-flow.yml` 1 + 1. `hypertrain-images.yml` shares too little with the
original `images.yml` for rename detection, so its history starts at its own commit; the
original stays readable with `git show e61461c3:hypertrain/.github/workflows/images.yml`.
The same command reads `gpu-vast.yml` and `release.yml`.

## Commit map (all 236 commits, newest first)

| # | CortexLM/hypertrain | CortexLM/cortex `v3` | Author date | Subject (as imported) |
| ---: | --- | --- | --- | --- |
| 1 | `679eb5b3b2665c9a81a54fc3f7332e75bbe53394` | `18ae364df450c062a72b40f100ed3c07657fcadd` | 2026-10-25 | perf(auditor): hash each dispute checkpoint and window once per party (CortexLM/hypertrain#35) |
| 2 | `0313989cfa05cb64280a0add9ac76b2c584c4394` | `279301faaf3bb2c979e91372c23030ffba643073` | 2026-10-25 | feat(challenge): serve a joined miner its own admission-trial job (S2) (CortexLM/hypertrain#33) |
| 3 | `add39503277e85a02deaea27cd5cfd9233a5d3a0` | `49e25de19dfdae40ffef67c23375278ea50632b4` | 2026-10-25 | ci(check-proof): run the bitwise proof gate as an informational job (CortexLM/hypertrain#31) |
| 4 | `95aa67533cf58cb5c20efeafeef520a55270fd93` | `66ab4dfbff666f21f4eb5c5e81cd78a2716dab2e` | 2026-10-21 | Merge pull request CortexLM/hypertrain#30 from CortexLM/test/bootstrap-pending-filter |
| 5 | `65f0a1845e71d13b0fab9ecec57003f90f9f8013` | `df42b22d7a60f8879df2c4f34f4cfff5d7856e14` | 2026-10-21 | test(ledger): guard the accepted filter of the bootstrap pending count |
| 6 | `9da3b7fb81c57709133374216df2cdec2aad415e` | `d1a97bc4dc5fe9f81fb442c4cae80460a4111cd8` | 2026-10-21 | Merge pull request CortexLM/hypertrain#27 from CortexLM/feat/production-bootstrap |
| 7 | `7c7346e5827a1fe209d14f79c6b574f4c052cc4c` | `a95da2c5b15364230c675b8b4409fe543be6f155` | 2026-10-21 | Merge branch 'main' into feat/production-bootstrap |
| 8 | `d1e273df0263714a10a08a5502cc5b83442da6c9` | `629fc581573e6798789518c204033583dcafbd1b` | 2026-10-21 | Merge pull request CortexLM/hypertrain#26 from CortexLM/ci/od-suite |
| 9 | `f8742e92ff3508bc6f4dd7176761ac94f8ada9b3` | `c980b0f5edd2a873d34fa18c0dc45b60af5caf8a` | 2026-10-21 | test(ledger): guard the bootstrap issuance cap at its boundary |
| 10 | `39de077c17aea5d7ae45c6c205d4bd49a446732f` | `dab000005ebbc2118bb8c1457a9cf858e1c95e24` | 2026-10-21 | ci(heavy): deselect the 14 authority-snapshot tests by name, log shard RAM |
| 11 | `3d154952cc34671ca5c66f6ef25fa520b2e41484` | `f978ffe72c04d146d1e27ac55423a17d25496229` | 2026-10-21 | feat(challenge): route production bootstrap and economic-bound writer |
| 12 | `d157cdfc4bc9b47c3b768b7a06e08feb63a3bb25` | `a29298704183e05f371b297f25118216523c057e` | 2026-10-21 | feat(ledger): add pinned production bootstrap issuance |
| 13 | `d362de139531a02620cc990620adb6c166546c95` | `283176073eef27deb3ba9b84dc38fdd6c36eeae1` | 2026-10-21 | ci(heavy): run the real od-base island tests in a sharded od-suite job |
| 14 | `bc33e53489852b6a5a6ea630c6f60f1a9be08ce3` | `e159f197078aa035c288fe9b4872a529d98ad03e` | 2026-10-21 | test(miner): give the real od-base two-rank launch a 900 s deadline |
| 15 | `47d843f3317b46a2f51c419a727135cc0ac77ef0` | `1c4e1241342aacdcbfb3f404df37d3116beb830f` | 2026-10-21 | test(od): keep the od-base island hook test within memory and time |
| 16 | `0f7f691fbe177c36a0958d6b83e87991b80096da` | `4093c8fceb6bc20460aac99b22807f0abcab4078` | 2026-10-21 | test(od): pin OpenDecision fixtures to the od-base counts |
| 17 | `f5f45331db3272a03549c36918ef20b4330b9328` | `de54430c74fdf784cbca29123048316e20827338` | 2026-10-21 | Merge pull request CortexLM/hypertrain#24 from CortexLM/fix/pause-v2-run |
| 18 | `c4976e291c0fe2e6564287dc5460c7e8ed036603` | `683ef4947255bcfd22cb1526bd3418490b7b0201` | 2026-10-21 | fix(challenge): allow pausing a v2 run |
| 19 | `a46ed651bb644ff70fed244683f1bbd4924a7598` | `2326b9b68a485ebfc9a719558c15d241df211229` | 2026-10-21 | Merge pull request CortexLM/hypertrain#23 from CortexLM/ci/test-tmp-leak |
| 20 | `7be66241c58d0d74f4a5a06b5ef32dd6a1c55842` | `119b6fbd717d6f7c6cd61cea14e56198403dfe16` | 2026-10-20 | test: import the notify cleanup helper from a module, not from conftest |
| 21 | `99c2c2dc0924376376104c60749945ade259cc69` | `c09bbfb2867c008278cb1c113314fc2100912d46` | 2026-10-20 | test: keep the suite's IPC and torchrun leftovers out of the shared /tmp |
| 22 | `dc163190b9dcb56d964be7a52d899f6b37138bc1` | `62e36a97d61457ac622938e3083867968da14e8b` | 2026-10-20 | Merge pull request CortexLM/hypertrain#22 from CortexLM/fix/role-dispatch-lazy |
| 23 | `6c9fda217c561689aa37a809d3709a530947e22d` | `fd8e5827ba03d6ceecbd0e05e0443b9302b08a90` | 2026-10-20 | test(challenge): cover lazy role-dispatch lock and unexpected load errors |
| 24 | `a493bd57922df9370080ec0260c55f2e87180ce4` | `ade8a0554da7a83b8d076cbdccae58410dfaa9ee` | 2026-10-20 | fix(challenge): load the role-dispatch file on first use |
| 25 | `2708b7d2635835fbac186b8307c28035b825d19f` | `a65a659c9a19b7f512564747f52028a685961357` | 2026-10-20 | Merge pull request CortexLM/hypertrain#21 from CortexLM/ci/shard-top-level-tests |
| 26 | `e6b00068bbe9a33fb47c082feca13839ed510624` | `9b7c890d878a4d044d631e1e8c336150e03f3d31` | 2026-10-20 | test(systemd): import the probe classifier from a module, not from conftest |
| 27 | `9e4630a3c9ccc3dc6f1333c731674de07d0f0b5b` | `503d5f150c4e35ea317b7fb97d05b07bb533656e` | 2026-10-20 | ci: shard the top-level tests/test_*.py files |
| 28 | `9bec353c1e0305a726722acfb90a0f2cd20dae8e` | `0ce89b314df23d9e065eabd167b1ca00146ae768` | 2026-10-20 | Merge pull request CortexLM/hypertrain#20 from CortexLM/ci/heavy-green |
| 29 | `e10b0540514451c14ae88bd0b3357b6dedebddf3` | `078a7e27033291ccbc0bc1c7c32f680094e8c828` | 2026-10-20 | ci(heavy): print skip reasons in the full-suite summary |
| 30 | `6544a94229f4f086cb04ed80da471d18a4dd1faa` | `78d2cff3d31b5a21afb9ed3e4a3953d76bce63b7` | 2026-10-20 | fix(tests): skip systemd probe only when manager unavailable or denied |
| 31 | `2ad33beeb4d6d3e107b47ebe029a7a015e01a7f6` | `f49974c378477a01825dcde34e4c9c0a475a9e8f` | 2026-10-20 | ci(heavy): install the kind version pinned in deploy/k8s/versions.json |
| 32 | `0f1840b37e4d5c99374106aac1713975763bc11b` | `ef8a90f539132b012b2d5afd93677bebea3dd3f9` | 2026-10-20 | test(capacity): skip system-manager tests when transient units denied |
| 33 | `a0555f30cf884733bad4a94a3080566275bcda4b` | `5cdd2ea2886b8c3d5c9e396c61857fe33fd3df2e` | 2026-10-20 | test(launch): reach preflight in the launch_cuda_run fail-closed test |
| 34 | `1f301864ba39281acc125b28100139289cd2d8af` | `9cabb9bdf9713dfe485a1e53a5fc9017c8d53e5c` | 2026-10-20 | fix(publish): allowlist two reviewed synthetic test fixture lines |
| 35 | `fc2f0eaf516056af06a1d7a68fe8579892b0cf5d` | `dfe69ab5aaa6ea4f6fe7422f65351f6e7125535b` | 2026-10-20 | Merge pull request CortexLM/hypertrain#18 from CortexLM/relay/hypertrain/ci-blacksmith--e7aa352b |
| 36 | `14068ddd64603bee7fe2a37711d7c4dba680f460` | `9272fa9729004ebedbb20c1202e1ea22193d9e55` | 2026-10-20 | fix(ci): keep the provenance-producing publish job on a GitHub-hosted runner |
| 37 | `ecd1d7f70bc11a8c4540ee3809d088303f79a44b` | `cf87bcdafd0af218e9e929ac4f5b9c9ee764b010` | 2026-10-20 | fix(ci): set heavy full-suite timeout from a measured Blacksmith run |
| 38 | `b9097b4de816b07f15350af1200389ba6035f5d1` | `528c880af4ee8dab234ba0a04b6c3c6cbe25108e` | 2026-10-20 | perf(ci): build images on a 16 vCPU Blacksmith runner |
| 39 | `d90a2d749227cb9f11ccc119c7b110ff786fa399` | `bdc65ef3e00358ce09b4d17912962b2418f6c160` | 2026-10-20 | perf(ci): run lint, test shards and test-slow on 8 vCPU Blacksmith runners |
| 40 | `c7789f4b9b4e0ffd4727d9e8d58ccea3aa7221b4` | `f413464ac1e002003732d4adf1502de33b470edd` | 2026-10-20 | fix(ci): run heavy full-suite on a 16 vCPU Blacksmith runner |
| 41 | `949b8dce7e719527bb176e74693c3cc1af1b4fc6` | `4e5102641c4924e51095b881943f73a15368b0d2` | 2026-10-20 | fix(ci): stop the main-push twin run that cancels ci on every merge |
| 42 | `4a7166ded055bdf7b4cb964b0bbae48f5c0f2f54` | `e4101776cd56e10e9b1939c04296f9dc3e753a6a` | 2026-10-20 | Merge pull request CortexLM/hypertrain#17 from CortexLM/fix/relay-receipt-rerun |
| 43 | `7f881928228fbf7c396ff3e91805969cc925ef3f` | `2cd9ec952a1a12c769bdfc5da783421b1e4d7c8a` | 2026-10-20 | test(challenge): bind lazily imported worker names before trapping tensors |
| 44 | `dc5f25f9d6852002dd14214acebec450b3143df9` | `e9009a2ff3869204655e5f8e7b886ba4d283ee89` | 2026-10-20 | fix(challenge): re-check for an accepted receipt before writing a second acceptance |
| 45 | `9f06caab83409dc3b0efde9c780297ad3f6ead0b` | `b181dafa6f4353da59d2ec83700b8b2f65174c1a` | 2026-10-19 | test(miner): fail the kill probe unless it hit a staged job |
| 46 | `0d8b27f34c5e5f26fba4e79c14fd2e724ed10e41` | `93b22fd0e93b52869d0b93bb6a48775477048c59` | 2026-10-19 | test(miner): require exit 0 for run-v2 rerun after UPLOADED |
| 47 | `10ed9288a437ee11d0721b125cf0c73e0ce13656` | `7e928ab2c0fa3a8e4e2d93dd1f5c35bf268e52fa` | 2026-10-19 | fix(challenge): answer a repeated relay receipt idempotently |
| 48 | `8bdbe08d9d6bd37fcbef67a5ea53da5e50f42b98` | `de4c1b74c46feb888615832ebc7179e231d1636f` | 2026-10-20 | Merge pull request CortexLM/hypertrain#16 from CortexLM/forge/p0-base |
| 49 | `86c4554d13997d1596d749460da8e955edd358fc` | `8aaf756e2a9d578ca5bcde16ad0bb132f36dbd1d` | 2026-10-19 | test(trainer): compare ckpt and plain leaf roots in one process |
| 50 | `7bac6aea8d0027f2ffb6f6572ff39e48ef0821f0` | `afac3affb5d23765912e80ded83bf4c776fe4517` | 2026-10-19 | test(proof): reject a golden xfail from any GOLDEN consumer |
| 51 | `d4abb0e306ccf38f1f24a3cb9d2b1c7ae2b1cde4` | `a407e434f83270569ae7eeaa86b503e7f9b4590c` | 2026-10-19 | test(trainer): route act-ckpt and fp8 golden checks through xfail guard |
| 52 | `91647824f8676de769b1ef265a47f2d824bad4a5` | `8828c780386dcad90ffbc19812a6ff6081d47466` | 2026-10-19 | docs(relay): deploy relays before the master |
| 53 | `8750a7453a3a1373e84ad0923c45b9811af83b41` | `31d779493c4eb080906c8c957e9b3c740c9f5d64` | 2026-10-19 | fix(challenge): purge stale staging temp files |
| 54 | `bc79fd361a1ef8121c77f931e922fda265ae8d3b` | `ce570b0aceb0affd6098061ac9745d90a147ff13` | 2026-10-19 | Merge branch 'relay/hypertrain/pret-mineur--54da7b66' into forge/p0-base |
| 55 | `994594c5101c819da689fa1f6967b7a4fda579f0` | `879460d83197faf3b567246f7b705703928c330e` | 2026-10-19 | Merge branch 'relay/hypertrain/job-idempotent--cad0035a' into forge/p0-base |
| 56 | `41b3ab99eba453cc3209e96cfca291d0d068f5bf` | `1e5f49b62b310b05d620e1309e49cdc8624bd734` | 2026-10-19 | test(miner): add multi-round v2 CLI mining check up to get_weights |
| 57 | `9c206967f80e083a212dfecd17b12b8119582deb` | `71666deef8d6f4d33a92a7050c2d267ec1c541bc` | 2026-10-19 | fix(challenge): make island job re-emission idempotent |
| 58 | `381c2ee61b471e842a672876093d11b4fce868f8` | `81a7d39f72dbaa6108f575593f1a920e062350a4` | 2026-10-19 | docs(deploy): say the activation example replaces the whole profile |
| 59 | `f44e19fb43d4306f779c035e33c47da755989c54` | `84ab8082e90b487a1cc6c7e09ce290c3562a4f5d` | 2026-10-19 | docs(mfu): state that no attention path falls back for dtype or head dim |
| 60 | `f29a7401335780b4a3d039891c140c8900fc3457` | `4cf7282dd1ef429f8f2f30003ee99394a27c5de3` | 2026-10-19 | fix(relay): refuse a replacement over signed chunk custody |
| 61 | `695b08030ec74d99da83a1ca922b76d3a2712258` | `ddff377e31604009c88a9f0efdcb5f4d649f96c2` | 2026-10-19 | fix(proof): report skips and xfails with one -rsx flag |
| 62 | `c18b68e450d2d8ff8fab7abdd4ce31cd5016386b` | `804a3769cc94be3e56f7ba4cd4c4671c0b03744d` | 2026-10-19 | fix(scripts): mask vast_e2e remote-env values under any flag spelling |
| 63 | `372767443493c08111174da522a90c22bf680e0e` | `af7d963a76974371952c73aa939b8c7281162bbf` | 2026-10-19 | fix(scripts): write mine_smoke evidence when World.stop() raises |
| 64 | `f6ed128fad1159d54bc13eedf5f3f0423e13652b` | `47f1fc549c45b8f2bec6ac9b1c33f1c0030c80af` | 2026-10-19 | docs(mfu): state that a rejected attention layout stops the run |
| 65 | `34b901077ec22f60fbbafa78e5f5e0ad26866533` | `4c989d8a19bbdf0a41a2d1f454c4ac5b6d87477e` | 2026-10-19 | docs(deploy): give hypertrain a positive share in the activation example |
| 66 | `e3e5343ecd910dea0b0ba7fe4ad60280cc8679b5` | `6927768041b3c1a3ef074555462c7eb0bcf9f4e4` | 2026-10-19 | fix(relay): let a master-signed grant replace an unreceipted reservation |
| 67 | `26229f41170d98619410c3dc11f8ab15802c42e4` | `6211d2ca11ab0f980c7bb7f88250a51a11954f87` | 2026-10-19 | fix(proof): fail the proof gate on a golden replay xfail |
| 68 | `ee6fa24a5e38a49ffa0ccba916b458607765fb1c` | `30d124a851a31209954f2b672e5856bb1fc2d75d` | 2026-10-19 | Merge branch 'relay/hypertrain/lens-hb-fixes--25a616b9' into forge/p0-base |
| 69 | `9db5d18a6baa827ab5c18a22e5f2c79ddc4d4bf1` | `e434ea9a4c8670ad4518e84678320a73dc43f6d7` | 2026-10-19 | fix(challenge): let a proven chunk list replace a live upload grant |
| 70 | `43cfcf78b49d5aebe5710bebe7461da471db6b12` | `f001175071466588ed04b7c7eb1fa987a8ad2ef2` | 2026-10-19 | fix(relay): keep recover() going past a transient entry failure |
| 71 | `369e69d1c079d693e28b951f873d63448c1f42ce` | `3a96334395191ec34223f3634857a9737f652f0f` | 2026-10-19 | test(relay): pin recover() isolation for store failures |
| 72 | `16c31964ae89ad18d76169fb933fef47daab18c0` | `1c201396e405511c20fe59d26ff9295ffee76dec` | 2026-10-19 | test(trainer): guard in-process determinism of the leaf root |
| 73 | `eae77e41c914701afdceb7aefe31e4774e1b0583` | `279bb841837914b5a9f376373073c30b17a40e8e` | 2026-10-19 | test(trainer): xfail golden replay only when the leaf root is unknown |
| 74 | `81e3150c58169f42f13adfc64b114058125f5a04` | `8caec41ff09dc921480ad9c8b3b975ec2da86b23` | 2026-10-19 | fix(challenge): disable a relay that cannot serve its signed custody |
| 75 | `730d248e288ac742325ad9323e34bdb8044e2b9f` | `56089529943447ad6b202a32d1571cf3d904f1d7` | 2026-10-19 | ci(test): log runner CPU model and avx512f before the sharded suite |
| 76 | `7fc35aa29ecdf77b069fc4a39c2257b50d8a7daf` | `4cb58e666d75972f33a03aceabdade8088fb263d` | 2026-10-19 | docs(findings): describe the v2 weights bridge as it now runs |
| 77 | `8a2538bf332674ea5f1e3a463c7debcbb36a394e` | `819a23dd40c9156fbcd83243913646ef16acf768` | 2026-10-19 | test(challenge): kill surviving mutants of the v2 weights bridge |
| 78 | `08b3d360ae7a1ebf33f042049de2d3e8b257ce73` | `b5a7d40c1e3b17cde7ee8bf769e1a9c387958514` | 2026-10-19 | fix(challenge): treat a corrupt held original as absent in relay failure |
| 79 | `e65ce46e4cf30e5c67fd583c08cd1cd86499d919` | `9a74741e1b0f92eaaf8902db9798fae281dd88dc` | 2026-10-19 | fix(smoke): reject an unknown MINE_SMOKE_V2_BREAK stage name |
| 80 | `cc81a17fac490037490f818d5408d25d513a340b` | `807ff55c3155e268d3c6efaad7f63fcd11bddcf4` | 2026-10-19 | fix(challenge): start v2 weights after the newest legacy journal answer |
| 81 | `a141d3fb32d118c004da2dcae5ea39bb06ef7e2c` | `b331a5c4c36a7b8a4982965e06065f9a00c3a463` | 2026-10-19 | fix(challenge): disable a relay whose deferred rescue it cannot serve |
| 82 | `6e9090373e8ea15548911a5dc7a54c26dae6d6a9` | `e50ee86dffba79cacb93998dc3b836bea384c1d4` | 2026-10-19 | fix(challenge): keep one live upload grant per relay assignment |
| 83 | `a78cda593ba7325bffb180621bf885375c3d2a7e` | `3ce3b4d0514ccd42e26009cb9441df525a050b73` | 2026-10-19 | fix(relay): fail unassemblable uploads per entry instead of aborting recovery |
| 84 | `daf88a679d11d6a304d970950708c567b7ca2e2b` | `d65b6a317d24d228ae163e6a6e70a25c0e42ebc0` | 2026-10-19 | Merge mine-smoke-v2 and v2 upload-grant/relay-disable fixes |
| 85 | `db94ff4d14d6748bd4b9a8df12af02e2236026c6` | `a382b55c35272e7b41f3db5d76596ae4d540fb73` | 2026-10-19 | fix(challenge): defer relay disable until rescue proves chunk custody |
| 86 | `37843776e5b13badd2e650e0d1378aaf6555a432` | `324530a1835bf724dea8548836c210e40d2b4815` | 2026-10-19 | style(vast): ruff format scripts/vast_e2e.py |
| 87 | `6b14d9965c042794eb0f8a290fb3756a9dc83a47` | `9294b68fa6cde013f0c1f622df4e75cc6f657ea8` | 2026-10-19 | fix(challenge): grant uploads whose delta the master does not hold yet |
| 88 | `ad617083fd171fd8b879d71ab0ab151ef0641821` | `4c339757b77623b9042852b651412759b5aa79fb` | 2026-10-19 | fix(master): grant uploads when the original delta is not yet held |
| 89 | `06c1e291cf0185cd130889ff34c30553cbe5476f` | `a956dccf238715c00cd210df9b0c0b5aa8ffefd7` | 2026-10-19 | docs(network-v2): how matured v2 rewards become get_weights weight |
| 90 | `6977052cd068f30ec8b2484b770a4f720f649a2a` | `f082daed7240775909485b9292487e84d3198279` | 2026-10-19 | test(challenge): v2 matured reward reaches get_weights end to end |
| 91 | `3efcd527bb35af1b40232249ed242ecdab0689ac` | `cdf507dd898904cc3e736d53d7118fb525f64b47` | 2026-10-19 | feat(challenge): pay matured live v2 rewards as get_weights weight |
| 92 | `a5adc59d5b9a133e2c7e639a415ce0e26e20f7ea` | `67b3f5eedec04badd23ac523d5967277bfbcb943` | 2026-10-19 | docs(network-v2): document the CPU smoke for the v2 miner CLI chain |
| 93 | `b364bc456e8ad61cd86911ef572e3409e815dd85` | `991c2ddc1c85a4f7e1e95ff986c816f11ec2c09d` | 2026-10-19 | test(mine-smoke): add CPU network-v2 smoke chaining join/probe/proof/run-v2 |
| 94 | `ac1f7d15231ffa10a7a2c6436d09d101622a807c` | `f7a27347fee3e73e2da1c5a0358f5ef6f4379c0f` | 2026-10-19 | test(challenge): prove v2 rewards never reach get_weights |
| 95 | `4006d7814f60e78bb8f3b27d6073c44a40375268` | `7ffbd5dcade29f4083f3fcc075a907ef3c3a7fd5` | 2026-10-19 | test: register slow pytest marker |
| 96 | `df824f23339589e3a270bdb13e48caa1ccd0683a` | `ec53f3442f7451de4d7aec39fddf084c9cdaa064` | 2026-10-19 | docs(operator): introduced_epoch must not be in the future; third challenge needs version 3 |
| 97 | `7eeb2ddc2dcbab4fb9fbfb14aa64025d6d1d568b` | `fdd7eca74a91159a5f6db55a14307183b9a7226c` | 2026-10-19 | fix(gates): check-proof skip guard ignores colors and anchors reasons; journal masks --remote-env values |
| 98 | `8e882ce1b49c15f1516343bf9a1986b3e3595b28` | `f6e5ecf515234b9ba778d8ec16d70360bde94163` | 2026-10-19 | docs(operator): no Cortex leaf key; the gateway signs and verifies nothing for hypertrain |
| 99 | `850a0c270c63ac02d3e351ccb1bc0290ed727abb` | `4e8c6dd8d2f0f398db7d47b926e6e3c1f1286a3d` | 2026-10-19 | fix(vast): dry-run docs and masked remote_env, proofs exit 4 on unqualified driver, keep price ceiling when paging |
| 100 | `48e877cb4240c079f43a7337221373504bd58610` | `54c3420f45159d310584a29839f5926d5aff6d43` | 2026-10-19 | fix(mine-smoke): an imposed evidence path is chmod 0600 and truncated only at write |
| 101 | `b42ce64765630eba92e1e02aa5ba205643f201bf` | `0cc3f61a4c88c8ff92729536384abe121626c72d` | 2026-10-19 | fix(check-proof): a skipped proof fails the gate unless its reason is allow-listed |
| 102 | `254c6eb35b1f0adcdb6ad4a53b773562738378d6` | `7e3cbf32220bb675a2c4ca10572667aca2e90fb0` | 2026-10-19 | docs(operator): drop the trust-root signing ceremony for the gateway-served challenge profile |
| 103 | `8606c8e0e123df5447ff4e1dcee89daf01e5e252` | `bc513ee2eaf752d2a2bfbda78661ef2667bb9399` | 2026-10-19 | feat(vast): journal the instance state while waiting for ssh |
| 104 | `89a7eea20c50cc688a6151441f62ae2043fc7400` | `5d2c588512149ecae9a50fc1378798e65dd0b338` | 2026-10-19 | fix(vast): proofs profile asserts the real attention path is flash |
| 105 | `7b23f9912eafa47691a155052651281f9691dfb0` | `634862ed87f80a0f33cda41ddb43c0c72bd45db1` | 2026-10-19 | fix(attention): serve only the qualified flash kernel under sdpa-flash |
| 106 | `64b79e2edba8f17e0fdd491707152c16a343d4ea` | `39c6d1a96e47960fcb6cc6a0b91ea376b3924a87` | 2026-10-19 | feat(vast): add --profile proofs, every GPU proof on one 2-GPU host |
| 107 | `90d5fc369e5e8b054c256c314d9f914f34770025` | `7c948e67b5715e05f3db5018f4542601862e7664` | 2026-10-19 | fix(vast): say why offer paging stopped on a one-price page |
| 108 | `791ff8f6130d333ee7a6dfd8483c4d0596ca93df` | `49452c8f72cfa01ca1a72253514cba0980ea4005` | 2026-10-19 | fix(vast): page offer search past unqualified cheapest offers |
| 109 | `1dde9bcbf2cb856abf9b3e97b69ed0df47657830` | `19404a6351fd981de1216eb18e361f7957153b9c` | 2026-10-19 | fix(vast): honour --exclude-offer in profiles that override eligible() |
| 110 | `d57020e2568530f701558950c3505f2ac97cc707` | `49dc23c64a109c2467c07e5fd96cc6f1801f3919` | 2026-10-19 | docs(scripts): point check-fast NOTE at check-proof.sh |
| 111 | `8e0aae2476f7d2a9a992241dbfb549881f53ecab` | `13b85a157617f3fb29bf494f471ec72cea43140b` | 2026-10-19 | feat(scripts): add check-proof.sh gate for the bitwise proof suites |
| 112 | `57d49ebb838361439ff4ec5303e72ca36fcc98f6` | `15476ab52a40df411a6663cfa6a72f08051af9df` | 2026-10-19 | fix(scripts): write mine-smoke evidence to a fresh private file |
| 113 | `811c23c95e793e532c54a075ea83703879ebe6ad` | `34dd8038643a8a7e7691d32a252ffea3e860dbc1` | 2026-10-19 | fix(beacon): redact path credentials and live-check relay output |
| 114 | `03a3f7d88a0efee9f845a030bd8748d5a38df34e` | `d46e3a6ce21b2b71fa83e08c8b1294948df22480` | 2026-10-19 | fix(attention): fall back when sdpa-flash rejects GQA inputs |
| 115 | `2bbf3f4d0a0f821e02354abbef080da71b804c66` | `d2153dfa52b1c6384f96423d93410d366f219246` | 2026-10-19 | fix(vast): re-prove absence when every host got stuck booting |
| 116 | `cc4e70e688a71aa33b4dda688bdfc06dd555f9de` | `236ddb003b64eab0e1ed2a1010bf64b26b07b10b` | 2026-10-19 | fix(vast): force math attention for the gpu-e2e fixture model |
| 117 | `e1f6c82ac3787cec2239daaa50171594c75d0d17` | `aeaeaef50d1928b731f4b22bfb1f7d72ade611bd` | 2026-10-19 | fix(vast): rescue every torchrun rank's stdout and stderr |
| 118 | `f5fcdf2abb40cbbe7185db77ee867ae2c7a33025` | `7be6ec8d0a877c751f5d0ef2bf65a0b2b90347fb` | 2026-10-19 | fix(vast): a stop signal cancels the run instead of aborting cleanup |
| 119 | `ef8eb7f1cc12ed4d5c9567b286c17f54f1b354db` | `fc4d662829e5dc1394a7e7ac358889ef67156fad` | 2026-10-19 | fix(vast): wrap sample ids so the gpu-e2e remote runs on 8 GPUs |
| 120 | `1a58e1a68a1cbcdef7df6b24b97fc5a091f01547` | `55ed5eb8edbaca80f65fa31bd70ce80174e86019` | 2026-10-19 | fix(vast): delete the instance when the harness gets SIGTERM or SIGHUP |
| 121 | `7dab20312859c30fcdb350a56a08194390d014b3` | `d8813c7a37ef4178f2b600a38591599be64fd95e` | 2026-10-19 | fix(ops): close six minor review findings in operator tooling |
| 122 | `2339eb1ba2d1708096b2fbdf4d6170ffee0d4f71` | `a7d4c577a4fa4285d320f1f1504d5d6662824c91` | 2026-10-19 | fix(vast): apply the driver allow-list client-side only |
| 123 | `18af21a2e442fbeb0ab92ca98ef804d0ebcdb0a3` | `3a4861433cb05623237ccd338e2735679b1ae21b` | 2026-10-19 | test: put the new operator test suites in the fast set |
| 124 | `ef74c1d2cdcf223522700b572c79b5df9dfed82a` | `c974fc9bd1af5620b8e3269f379af2b170ae4e81` | 2026-10-19 | merge(miner): two-step join-v2 and escrow-lock (lot d2, ses_2757f31f) |
| 125 | `1859c33935c87bdbbfc05d206cba3de362f6958e` | `24e49a769a753ffdf6cdca09170f53ba16c6b94e` | 2026-10-19 | merge(challenge): export a staged admission trial (lot d1, ses_334e1179) |
| 126 | `598fa0f015d0cf42bef74a53bd7193beba517bd8` | `7da800eb14a1614afe01cee736e1f98240eb395b` | 2026-10-19 | merge(vast): num-gpus, driver allow-list, miner-cycle and gpu8 profiles (lot e, ses_761f8bbb) |
| 127 | `0b224840f95e7e6340f54ecfd2854644d8aa72fa` | `4c9f51fbfef76209feda97f57e914525da7ef697` | 2026-10-19 | feat(miner): two-step join-v2 and escrow-lock without a coldkey on the GPU host |
| 128 | `67032c751c3087760a7c84566f83d1dd28fc90a6` | `e1e437945982d33c278d7a11fe1f311dbd3092b0` | 2026-10-19 | feat(challenge): export a staged admission trial as a copyable folder |
| 129 | `725682c742b986886396de3fcba565d5d9c86873` | `57ce0e55b3ca618a136158e2e1aa8f6e3194f008` | 2026-10-19 | feat(vast): num-gpus on every profile, driver allow-list, miner-cycle and gpu8 profiles |
| 130 | `a383777cc1af7cffa4a7d2853219e4c0fa122c0b` | `130264b02edb592ecef80303a08a818adaf4b9db` | 2026-10-19 | merge(datasets): pin and register contract-2 dataset objects (lot b, ses_70ab2972) |
| 131 | `033c7dcac6ac68719332d6d351e5da6bc16e73fd` | `a2863a99081bbbea3aeb623ac627b9b0611ed099` | 2026-10-19 | merge(ops): operator CLI for RunManifestV2 and its objects (lot a, ses_ac34a486) |
| 132 | `6adab563b71d2fe0ab074734cb5464d66dfcd352` | `6ba39c47c9ee64efc026447fe28e2137c24f871f` | 2026-10-19 | feat(ops): operator CLI producing RunManifestV2 and its six objects |
| 133 | `78a9cdaeb63dd3fcea8cc247f23a0860e1a3c3c8` | `333ec3731bf03d432516943c698861bc86a6dc58` | 2026-10-19 | merge(beacon): operator push command for drand rounds (lot c, ses_b2396eb8) |
| 134 | `d8cd9f87496df662b1a7210fcd9f1f0fa81666ae` | `025b4d4652788b2e0314cd75b4dcd31ad3c6ad51` | 2026-10-19 | docs(datasets): replace missing fetch_data.py with build, pin, register |
| 135 | `334f96fe158dcd4f0797afad759f2d496cb866cc` | `2762e891b256c553664e48d1d4c71c2a25795c79` | 2026-10-19 | feat(datasets): add pin and register for contract-2 dataset objects |
| 136 | `4a7a9d0bc4427500aad7a06b287fd57dedae3a7b` | `ae1a0833245ab68fed542b0cc186cdd17ca19611` | 2026-10-19 | docs(operator): document the beacon push command |
| 137 | `1a7cade720caea6c8fe1d306f863cae8711d3a09` | `894012269b19ead9321e36e6ce6d969e17d9d567` | 2026-10-19 | feat(beacon): add push command feeding drand rounds to the service |
| 138 | `222e966b7475611e76e59e30b59de13ca8d5b87a` | `82fca926ecc2495cdf30558cec50952cb72d7e2a` | 2026-10-19 | fix(vast): type the tried-machine set so mypy passes |
| 139 | `688879225555454be652a5a7728c30d0d3cd722e` | `27da4db183cbd26d9d6658b35db37617c9532d18` | 2026-10-18 | test: check-fast.sh collects only the fast paths; mine_smoke typed for mypy |
| 140 | `98a96536e94be39e84f18cfd5bcdf71f8e7e1af8` | `ba44bb078eeab832c345ad88b2561993c3b3d86e` | 2026-10-18 | test: fast marker, check-fast.sh and CPU mining smoke under 2 minutes |
| 141 | `1818f9e7247c8a194650dd62c53c4c89c945d157` | `5c0b8b290628cfa7c60d52e0bf552f7928bd7a85` | 2026-10-17 | Merge pull request CortexLM/hypertrain#15 from CortexLM/fix/probe-hf-redirect |
| 142 | `d6fc70b8a55c9d75a6a4b246d77f8ec2eff95fc6` | `21d082a8ee60d612bb62d533f2b97d89217ffba1` | 2026-10-15 | fix(probe): follow HF 302 redirect when measuring host ingress |
| 143 | `1df4832f572dd5612916cb403816536f2f57c33f` | `1a2b994a99405e031e45ad42511d142dab96efbe` | 2026-10-15 | Bandwidth A-Z: hash-only rescue, theta-only miner, lazy trace, H=512, real FineWeb mix (CortexLM/hypertrain#14) |
| 144 | `c372ca78b8ca9cf1f90d719badbb1a5c6aa12fca` | `2f3e0090eab1db7a20c0ef63c5a549b17a5affb7` | 2026-10-15 | fix(qualification): detect hash-only rescue by tensor absence, not any file (CortexLM/hypertrain#13) |
| 145 | `419ec8baadf491f88300b81d8b735398218b20ee` | `457ce90c1f850088178e6107ef5d1ce26f754949` | 2026-10-15 | feat(qualification): hash-only rescue per rank before workload (CortexLM/hypertrain#12) |
| 146 | `63409f5dff9e3f994fcf8a202c7f50fc4501a5a0` | `fdd36a5e13f644a8500f1cdde8bceead3eff2f7b` | 2026-10-15 | fix(launch): run every stage on the frozen controller tree (CortexLM/hypertrain#11) |
| 147 | `83d79148664de763af8a8081c9c209248deaed0a` | `15378bfa45b14bdd06d9e71c8d6b4a1a958890f0` | 2026-10-15 | fix(launch): restore staging time in the signed launch window (CortexLM/hypertrain#10) |
| 148 | `2fb183bc04497e5886886f854e9de86b84b80cdb` | `94c2fc3472b764c1481c4ee19b05b73f91b895c5` | 2026-10-15 | feat(launch): minimal signed window, mine on qualified h1, admit as runtime uid (CortexLM/hypertrain#9) |
| 149 | `bfc63eb50d345bf2a8d687c20f6336afd34d6ca4` | `55b7c3aeb1ee0eed53493b2dc3c0553a1c21ac17` | 2026-10-14 | fix(miner): 8x5090 launch path - sized PUT, 5xx retry, parallel fetch, one-command launcher (CortexLM/hypertrain#8) |
| 150 | `1a8f91e887b48952ec1adade175ad9efabaaf908` | `adf23cd6e63ca76e898d4a07ff9c528e92ecd5cd` | 2026-10-13 | feat(od): activate od-base, remove od-tiny (light CI) (CortexLM/hypertrain#7) |
| 151 | `f0ccdceaf4928b9aad3dbae652491e7ad76f61c7` | `733d25575e1fff26652e7a9639ce263eda1c70eb` | 2026-10-13 | Merge pull request CortexLM/hypertrain#6 from CortexLM/ci/fast |
| 152 | `fda72aeffb7538efa8292baa3547a62d2a81ce7e` | `567c87a7da4fd5a500b42ff451db3b06f7bcc5af` | 2026-10-13 | ci: 7-min PR pipeline (lint split, 8-way sharded test, heavy suites to nightly heavy.yml) |
| 153 | `e07ac0ba1e2f970436ce7d7f61e0e097e7a96cf6` | `d66cf6a098d126f233965f9b5643ac528dbfd5aa` | 2026-10-13 | Merge pull request CortexLM/hypertrain#2 from echobt/pr/mfu-multigpu |
| 154 | `b7bf62eb9702a4f406f75dd9fcfee708bef3a4e1` | `b70149c7865298a5a8559619b05f33e739ffff8d` | 2026-10-13 | Merge pull request CortexLM/hypertrain#5 from CortexLM/ci/speedup |
| 155 | `f4e750d6702c23a0649d09e4ad25350974dea136` | `7b98392b0e3d8964991c22b9fc316c29c5fc5873` | 2026-10-13 | ci: speed up pipeline (uv cache, concurrency cancel, shard test-slow sim\|e2e, reuse native uv cache) |
| 156 | `6f7136b9f212fccd76367880bef1fbe9371312a9` | `64d744972d8699919a6c412766246ee92f9e56a4` | 2026-10-13 | Merge pull request CortexLM/hypertrain#4 from CortexLM/e2e/role-dispatch-delivery |
| 157 | `a811497940380e8ff1ae2f78d41505a1e4b3cb80` | `51855fdc61857a92406158652ad69b0481b6a1b9` | 2026-10-13 | fix: restore confined audit input read; default artifacts profile; align test fixtures with put_large contract |
| 158 | `24b769c2811a49b2b0ca31db6dfc360137b29019` | `d58a45aaf1e3c11d5d73dd40fb2ed61f19d5471e` | 2026-10-13 | ci: fail closed unless torch CPU capability is AVX2 with one thread |
| 159 | `3653ebbdb3a7b360bb6b7f3a8a7d2133c45c28ed` | `fda32fc9c09a1843de0dbe9473c3ed3c56739143` | 2026-10-13 | ci: bound CPU kernel dispatch to AVX2 for bitwise golden tests |
| 160 | `846b94819e365c1297d3349c5b75e74bea20f35a` | `7dae3ee79eaf9a2a75b6c08d69891b1ca28e2718` | 2026-10-13 | fix: key StartStateV2 objects by the put_large stored ref |
| 161 | `0a5ddbe30be35898967e7c016b13f0b4d19dbc3c` | `6ce3d5f6042192d3ffe4d63ee60f0ba4623cba7a` | 2026-10-13 | fix: read indexed ArtifactRef objects through the part-index contract |
| 162 | `818b20025943d9c7f2d56bd74827148fe51d57ca` | `b12f9e9ff954751c7a41accea279a0b3da64b327` | 2026-10-13 | Merge pull request CortexLM/hypertrain#3 from CortexLM/fix/beacon-pairing-cache |
| 163 | `12818852941fdda8a737cd9e8d7293e2dea4a806` | `41ae1ad673ba64ff05dd7314d82f3d698ad1b4c1` | 2026-10-13 | feat(gpu_ops): role dispatch over owned GPU hosts with reviewed two-stage launch |
| 164 | `3f937dd4d88e15743f41dad1f75984854312d6c8` | `dff59ac20ee783bd9310941d61884e694d640e66` | 2026-10-13 | fix(beacon): memoize drand pairing checks by exact signature/round/key |
| 165 | `4426f8b914d89cb94b69214af405163ea5ca4695` | `130de35f4b579b09a12cfc95517572a64736d313` | 2026-10-13 | docs: multi-GPU training and MFU guide with measured results |
| 166 | `77d48567c66c5b362fb9c28a61e39d645eedff49` | `a0d0b629c782d91082d776aff4e9bed3217fd134` | 2026-10-13 | feat(vast): --profile mfu for N-GPU MFU sweeps |
| 167 | `164f49d9e13c03fdbc54a87a1c5c5745bd3e162c` | `4600911c981c61b41e2800671d20178859e176a4` | 2026-10-13 | feat(scripts): multi-GPU MFU harness: trace, NCCL PCIe, micro-batch, step peak |
| 168 | `91c6fbbdd2778065856877a6a1b5706c8d47a358` | `88dff681e54a6d9844c5494370dc66c0ad40b61b` | 2026-10-13 | feat(trainer): gated unit-order reduction, ht-leaf-v2, hardware classes |
| 169 | `2df4b77691f072092c33be17431a4ad93a9fca28` | `f23d753c8f6d277c3dc2157e48faae0e680233fa` | 2026-10-13 | perf(trainer): overlapped zero2/zero3 owner reduction, sdpa-flash on sm_120, RoPE cache |
| 170 | `e319cf6b1eb8325ed1258185141dfd073163387a` | `926a1426a72fe7ef14ab96cc8c4a0e4fec5cba91` | 2026-10-13 | fix(ci): restore phase-b experiment fixtures; drop allowlist entry for removed report |
| 171 | `1dd0dc48d3f2ad516dcacf9e29fa0aa14266bfd6` | `51dbb05709158ea08183e018ef41a78fa6993646` | 2026-10-13 | fix(ci): restore experiments code still imported by src/tests; server image ssh client |
| 172 | `70447b01f60f5ab06f8170b26cee2f8829027814` | `701fa5f1186ec9b2ccca1fd3b5aac92d8b30e0af` | 2026-10-13 | chore: remove audit/report/plan artifacts from the product repo (CortexLM/hypertrain#1) |
| 173 | `45ae2828b4c541fc10953d0b20511db4f570ad4d` | `35ac20ea98249cef3dbec9bbf868e08fb25d15fa` | 2026-10-13 | docs(result): GitHub ci green on 4c08270 (test, test-slow, canary) and miner-flow |
| 174 | `5a15a575abacb3a92e3e51836596e26b6251dcf9` | `440a7b3c69c8d661b6cc051f765c47d934138d18` | 2026-10-13 | docs(result): follow-up CI fixes, Actions evidence, Vast instance absence wording |
| 175 | `08ce9b7d2f43ae9ad935ee997486e37d5eed9334` | `4c0827078909311e650b35ba3d092d6f03064622` | 2026-10-13 | fix(miner): cli.main catches only MinerError/UpdateRejected again (SSL errors must propagate) |
| 176 | `7566f86d8e964b005db05d99b64bac644591e325` | `cdc6154bb8affa70d31c79456eb12507e2a47cc8` | 2026-10-13 | fix(ci): per-platform golden leaf roots (dev host + GitHub runner), sim reads device loss once, allowlist release key temp-file line |
| 177 | `e0c7c9b96b641232d785b92885b7a92930cd2b6d` | `b999d7017394f22e4290f1714d9223981deb5882` | 2026-10-13 | docs(result): local gate findings (sshd-less host, capacity timeouts; pre-existing on CortexLM main) |
| 178 | `ff33e577569847a0d895cefd7182de1692f2e57a` | `4add0e9bdc7045769983189ba65b8939a3a64f38` | 2026-10-12 | docs(result): Vast miner-flow run PASS 19/19, replay MATCH, instance deleted and absent |
| 179 | `553abdc0d607132f96eb00a3d913cdc5d7287035` | `b4e6a1cda5794e13b1c12171d23d7cc5a7792006` | 2026-10-12 | merge CortexLM/main into miner CLI/update work |
| 180 | `bd4e072771868fb5f5c41f1c0463d9755948d910` | `cd070fb924730328c6ff3c0556fcb06849aec205` | 2026-10-12 | fix(vast): miner-flow remote runs scripts/miner_flow_e2e.py |
| 181 | `3b694a1cf5551aeea56773256a24cc45558a7652` | `5f079f4479660d6a3dab4089c17bf38f6df247af` | 2026-10-12 | docs: miner update/incentive result and plan.json |
| 182 | `5f56b93e116daabfe45abe1b58bd4aa6170ceeb9` | `5024e5278b924e2e48760a383c064b02f3a33d51` | 2026-10-12 | ci(miner-flow): CPU e2e for seedless install, key revoke, synchronized update, replay MATCH; gpu-vast manual only |
| 183 | `1cd30cc6e797cb004b4c6b2add405a281e49773b` | `aca7c732ef6dc4de320bd2bda784b3841b4bb14f` | 2026-10-12 | wip(miner): CLI v3, install-miner.sh, ledger audit fixes, incentive audit doc, plan (unverified) |
| 184 | `c78c5c3d3837019352dc796ea793849e38f4f1b2` | `926d0b4e28ebb36fdf910dc1513898e4f2ca0348` | 2026-10-12 | feat(miner): signed-release auto-update with announce window and synchronized changeover |
| 185 | `e7ea754a16b15601b67ee42e9f197e36aa0ffb42` | `c1607ffe773dab0e8c91591188e50010a5e3bca9` | 2026-10-12 | feat(miner): coldkey-granted per-server revocable keys (create/list/revoke, 403 on revoked) |
| 186 | `3418cc5689b7477517e8d34437fdf2d13af00616` | `ad6f2f9950783830ab6f6200c50d9cd0f710f878` | 2026-10-12 | feat(vast): miner-flow profile (1x RTX 5090, vastai/pytorch digest, inet_down>=1000) |
| 187 | `d6b11be647a7f7533e27a70fd1112ccdd8760524` | `7bea9b610d7f1110e1d64e7970d46cc4dc4fb6f3` | 2026-10-12 | S6/S7: run_mfu_dist.py torchrun MFU harness (zero1/2/3, HT_ACT_CKPT/HT_COMPILE flags); check_gpu_evidence --kind mfu accepts dp_size + per-rank roots; bitwise tests worlds 2/4/8 |
| 188 | `5fa3e9dd9759d071f27cda9bde5ee5fcfe0bb778` | `51d4fe7e3994d27f29c3387eaf0f6704fd9ab896` | 2026-10-12 | S6/S7: mfu-dense-1b-dp2/-dp8 profiles (dp_size=n_gpus, zero1 owners) + tiny-cpu-dp2 smoke |
| 189 | `efbd04b12982acc7ef66e24bc4ccc30db22abfeb` | `e34f7f6a145a835b945d22f9577f794d8c22d554` | 2026-10-12 | launcher: connect over Vast direct ssh (public ip + mapped 22/tcp), proxy fallback after 60s |
| 190 | `dd73beb5b208a42f85a4ba72c4022a8ef3b4d12b` | `eb8c21020c5021a3e1ac048385a609773f873418` | 2026-10-12 | genuine_cuda_ci: prefer US hosts so the measured rescue rate from the runner clears the floor |
| 191 | `b873f2ee82a4ce45f86c3bbe7e111952836d0821` | `5683065d9201b3516f70652fbcd7833e4f744a60` | 2026-10-12 | vast_e2e: journal ssh probe errors and re-attach the key after a publickey refusal |
| 192 | `cde9e7bb371ede9e50d8116801ef75b8718250fb` | `b74fd0dcdd8e050e10aa6a491d54d500e5662166` | 2026-10-12 | vast_e2e: exclude two hosts whose image pull repeatedly exceeds the boot timeout |
| 193 | `7fd885ecd220b83e52e9378ad6d63a6a03a2c3bb` | `b44a95a7790155891824b206e54be3ae5d3b9745` | 2026-10-12 | gpu-vast: install the trainer extra so the genuine CUDA phase can build its seed |
| 194 | `52cdf2ce2878c53ca700c0878e54486fe9e01d26` | `10adfdae3613b20faf0264363cbe810851e4d3f2` | 2026-10-12 | S3 + S4: HT_COMPILE=1 per-layer torch.compile (single gated site, forbidden-construct test relaxed for it only); FP8 rowwise linear via torch._scaled_mm behind HT_FP8=1 / precision_policy fp8-rowwise, off by default, stub fallback |
| 195 | `2e3336b4ca818ecee3f3a4b898d397faa7e0eb26` | `99c75b8d26069b1aeb23cf22b71522653e5cc7c1` | 2026-10-12 | S11 + S4 manifest: outer.codec_version (v2 payload via encode_v2; validate_payload/decompress accept v2 so miner, aggregator, auditor paths decode identical v1 bytes); model.precision_policy field (default omitted, run_ids unchanged) |
| 196 | `ee7e8abed75eeda7143663bedb4d1135f34570f9` | `06ef05dc8e4ee1e19cf695986e5f7c849fd55532` | 2026-10-12 | S10: reduction=overlap issues bucketed reduce-scatter from grad hooks during backward (bitwise = bucketed); async checkpoint writer in island_worker (drained before outputs); pinned non_blocking H2D in _load_batch |
| 197 | `7dc234010588e5843b20c6730f388f2e670e10ad` | `2d790d2cf4249ba46e7a48b28b37d76ab0be657e` | 2026-10-12 | S6: ZeRO-2 grad sharding (all_to_all to owners, rank-order sum) and ZeRO-3 param sharding behind reduction=zero2/zero3; DTensor Shard(0) layout helper; bitwise equal to reference at worlds 2/4/8 |
| 198 | `b7cc7a850089febd687ddb1ac52f4882d41f49b0` | `acf163aa6fa3b1db67d4f9880987c6784ef37d8f` | 2026-10-12 | Parity grid: full 150-job run on RTX 5090 with batched CUDA simulator and CPU/CUDA equivalence |
| 199 | `334b17121aefd7af9292a4eee68942d34a09ea76` | `34acaf9e09eb1ec0005959dc51189942affe9163` | 2026-10-12 | genuine_cuda_ci: count other instances on the shared account as liabilities instead of refusing |
| 200 | `d7aaa0c65814aa1ff67cc050bfe0062416ca7a87` | `5e85129ddee9cadfb363277fb847fdbf7c5de347` | 2026-10-12 | vast_e2e: fall back to the next offer when CREATE finds the offer already taken |
| 201 | `86adcddc45111bd0fa2a70d9bf4f3d9551ac6c5c` | `aa62037b9f719421cf76091a04b76cee0904a16b` | 2026-10-12 | ws1: HT_ACT_CKPT=1 per-layer activation checkpointing (bitwise equal to golden roots); 32 GB profiles mfu-dense-1b-mb2, mfu-dense-0.35b |
| 202 | `c9a4f3a54ff7eaa644cbaa2d000134774ec212c0` | `ea3fd2fc1d663f6f9a9aa8f9b1f632c062fdda40` | 2026-10-12 | vast_e2e: replace a host that never becomes ssh-ready with the next eligible offer |
| 203 | `2d6d55c9990062046ab713fa2c996f626b79ebe8` | `12eb0611a86eb04bc88f45d5a9847bb5ad049d58` | 2026-10-12 | Genuine CUDA authority on real RTX 5090s in gpu-vast CI; orchestrate.py staging-resume and IslandJobV1 import fixes |
| 204 | `3eb51e934791b5d9e7beadefc95da7185265be28` | `4b2382707801b43032ac092ec5d3360c11f12014` | 2026-10-12 | ws1: run_mfu records HT_* engine flags and attention backend next to the leaf root |
| 205 | `a47fd9a71dc0ff7e0fb3029613ab201ca4fa088d` | `48482c50250c10c70f9d0b985cd0fc87ecdf141f` | 2026-10-12 | ws8: explicit ARCH_REGISTRY (od-encoder -> hypertrain.models.opendecision), unknown arch rejected; CPU from-scratch OD round bitwise + replay MATCH |
| 206 | `a80059474c3bd54dadfd5dd2a70717b0d3861b15` | `4325e86044b504a272df4d3cdb6cd863d0a853dd` | 2026-10-12 | ws1: opt-in grouped-GEMM MoE (HT_MOE_GROUPED=1): static [E, capacity, d] bmm, dropped tokens skipped, no host sync; default path unchanged (prompt 1.8 / plan 1.7) |
| 207 | `f02e9493341d2741bd8fd3bd4266e3a709bdcdf2` | `9e30cfdb0ecd307cbfa3e2da41c7fb6031d62227` | 2026-10-12 | ws1: bucketed reduce-scatter (all_to_all + rank-order sum + all_gather) behind reduction='bucketed', bitwise equal to the all_gather reference (prompt 1.3 / plan 1.4) |
| 208 | `cb4407c9acb897bdfa064abe40a5cb8b5d54c092` | `caadd3d25e8cfdd457c2e61185e2343de68778c4` | 2026-10-12 | ws1: no per-step host syncs: grad norm, clip, loss stay float64 device tensors, read once per leaf; bitwise equal to the float path (prompt 1.4 / plan 1.3) |
| 209 | `6e55c73121759709adbfa5d00ae5ed497cd72774` | `451bb266f926d017e7a5b72d5fac3e19f079af69` | 2026-10-12 | ws1: multi-tensor fused AdamW (foreach) and Muon (shape-grouped batched Newton-Schulz), bitwise equal to the reference loop (plan 1.5) |
| 210 | `05a1d35237f0aa88bb5df16b0c866206b8a37a16` | `a551034e187231069220a4dbb4e21a92836a953d` | 2026-10-12 | ws1: HT_ATTN_BACKEND override, sdpa-flash CPU repeatability/tolerance tests, golden leaf-root guard, backend recorded in run_mfu (plan 1.1) |
| 211 | `54477aee8af0a9722e132dd08e36e38b9dfa8a10` | `a65b8ed7282def5ca149b7f0a0b9fd791f365d5b` | 2026-10-12 | ws1: mfu-dense-1b profile, scripts/run_mfu.py harness with replay leaf-root, check_gpu_evidence.py --kind mfu recompute (plan 0.3/1.1 prep) |
| 212 | `f8ed521e2e6e0b9be90da9ff25ce28d99c863313` | `384e20b49f4d2f67578d220b9f0470ebe46f88b9` | 2026-10-12 | ws1: attention backend selector (math default, sdpa-flash/flash-attn gated by spike qualification), native GQA API, cached causal mask; GPU spike script (plan 1.1 prep) |
| 213 | `b7fecfaf3a0d26a1bdf495e5db0614223365fadb` | `467a6a35f1359c87f42999eb6d2d16abbf9e7a8b` | 2026-10-12 | ws5: k-of-n auditor quorum on reward items behind auditor_quorum flag; distinct coldkeys, mixed verdicts escalate (plan 5.1) |
| 214 | `1ff642c2452ee7b3739442acc2676a77f4f2e4ee` | `a507c9d7355e1cdd4f2994b2982a520912b7cb36` | 2026-10-12 | ws4: roster cap behind max_roster flag (default 32, ceiling 1024); explicit guard error instead of assert (plan 4.3) |
| 215 | `55a394d68d7dbcd1c87464508761dfba051cbf94` | `d5ed093384606b6c6581179ec21526a04d0e538a` | 2026-10-12 | ws4: ht-norm-v2 fixed pairwise float64 tree, opt-in via norm= on clip paths; v1 default (plan 4.2) |
| 216 | `7ea7a052473176511d1ed062d0dd1e60e9cb70db` | `82ba4e8b62d88ab44152ea56dba38b430720712f` | 2026-10-12 | ws4: vectorised exact ht-norm-v1 sqnorm/dot, bitwise equal to fsum (batch2 task WS3 3.1) |
| 217 | `7f3acb5473495c4ed61e64e28e08dce8756f41a9` | `95ba66bf0f1114c17f1043e7cc37522beb8ed4f4` | 2026-10-11 | ws2: codec v2 lossless container over exact v1 payload (plan 2.2-2.4) |
| 218 | `b072d5764fec0b832361984e1457bfc4a8711872` | `7666f9c71099249c2ed40a022e48d9d1e6bdd786` | 2026-10-11 | ws0: FLOPs/token and MFU calculator (plan 0.1) |
| 219 | `4afc4ea6d44da16bdedb1ac12bddc37f26e5d2ce` | `e9cba7f3413d9e855eb81198ed42e04e8820dcd6` | 2026-10-11 | plan: review amendments A1-A5, owner decisions approved |
| 220 | `d80b97883630d15c64a6637bc25fbb2062ea2d2a` | `dc4681e43741a7afa5fa483538683123d8c883e5` | 2026-10-11 | plan: A-to-Z improvement plan from audit 224a523e5f92 (MFU, lossless codec, complete network) |
| 221 | `8baf7c869f80074455fbcfcf894c302e198e8ce5` | `7389b7486a13ac1dc1618af7b223eb8d1825694b` | 2026-10-11 | gpu-vast: name the key path variable so the publish scan sees no credential literal |
| 222 | `783dcb3d05cba566a60672022b14c445b6116afe` | `af4bb25f626d5ebb531e028df8ac0be4d794428f` | 2026-10-11 | Add GPU CI on Vast.ai (2x RTX 5090 end-to-end after green CI on main) |
| 223 | `ba82c56178e7d9d68bd3199294051b3ad0396725` | `a850202353a1e3f0b7f724ae74c7d45239a0ee64` | 2026-10-11 | Fix test isolation in CI, VRAM probe without temporaries, multi-GPU Vast e2e result |
| 224 | `b1a81da3f8b5bd7f2b4ba2bf3152c75adf816ad8` | `0d416a6dfae1f376bb81a5f022073f727e052a1c` | 2026-10-10 | Fix CI: formatting, get_weights without payable hotkeys, canary script mode, self-contained checkpoint fixture |
| 225 | `8c072448412eec7e68c47aca1d543cb5f64d777b` | `a7489c3c0b58b6acf6e225177f565e95eae915e9` | 2026-10-10 | Multi-GPU islands, subnet 100 permissionless admission, multi-run service, autonomous disputes, GPU compute self-check, strict secret scan, Vast e2e harness, new README |
| 226 | `802c792f558d51e8f357fba69dbcc8b135acb31e` | `a8f5028bcfc17ae9b4b25baea13d495ad2622873` | 2026-10-08 | style: format PUT pacing regression test |
| 227 | `7eeb2a00e1c8c763108197cc00aab54bc4857a53` | `87f19d5692627689fc223527e8c941c955980fdd` | 2026-10-08 | fix: record PUT completion time independently of journal latency |
| 228 | `f92a3a9c1bcedbf968318c2845bb66c71f19e100` | `22a422a43116596cd6d285f288390843419c0c21` | 2026-10-08 | test: remove idle HTTP reuse race and await server readiness |
| 229 | `e67ded407de0ad3be60fa5444fa03a9c5a37e5ca` | `32a420db69da786b116a837bc2ddfaa36e9df3da` | 2026-10-08 | fix: bound CPU collective benchmark memory for CI |
| 230 | `a6f4157fe943fe2ce5f25823b66eb16879c1c903` | `072ed6a2de505b6d416264422514f4f378c75d99` | 2026-10-08 | tests: pin conftest import order in auditor qa_scenarios (ruff isort) |
| 231 | `7c6151266d998526f8f5d11909fe2c526144adb7` | `b0f00eeda75d615339ca89d952e93ddf839aadab` | 2026-10-08 | Hermetic tests for CI runners (tiny corpus fixture, non-root mock sshd, tmp evidence dir, canary secret perms); GHCR images on push to main |
| 232 | `8a0c10e5aa73ba146585c818def45e069a49e9dc` | `9f7d56b4281a1c447b8298ed64da4ac8fbc8a706` | 2026-10-08 | CI on push/PR, teacher labelling (OpenAI-compatible, OpenRouter DeepSeek default), Cloudflare R2 publish/doctor |
| 233 | `70066422d16628b76418152dcdb65e88eca9c271` | `a148d24005fb6132479553f509944488edd9982d` | 2026-10-08 | OpenDecision runs: od-encoder adapter, u16 dataset registry/shards, /public API for subnet-frontend, OD disputes |
| 234 | `de9eb61c498f4134451a9d4abb25a5c6fcd82ae7` | `3ee2fa90572581e00926d6546afe7a8c85ee1781` | 2026-10-08 | Results: Phase B GPU determinism, parity status, RESULTS.md; lint/type fixes |
| 235 | `c7e1459cda449ab4ff362db5d712f80951bb8217` | `9783584a6ec1861a8080fc01abfadeb7a9986e27` | 2026-10-08 | README: badges, research disclaimer, compact diagrams; publish.sh emptiness check |
| 236 | `03de4fb23806a6342969f5ebb2cd224d1c40a3af` | `728e693e1f43b83fdb96bf44055809c660ba481d` | 2026-10-08 | Hypertrain v0.1.0 |
