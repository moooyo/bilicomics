# Ordinary startup cache maintenance: six paired comparisons

F is the original fixture text; R contains the same lists in the product's persisted text. The executable is unchanged. Each cell is elapsed/thread CPU in microseconds. Negative R−F means less time on R. Two warmups are excluded.

| Pair/order | Parent F | Parent R | Parent R−F | Commit F | Commit R | Commit R−F |
| --- | --- | --- | --- | --- | --- | --- |
| 1 FR | 7300/691 | 473/466 | -6827/-225 | 6850/247 | 10/10 | -6840/-237 |
| 2 RF | 7919/936 | 496/490 | -7423/-446 | 7375/393 | 5/5 | -7370/-388 |
| 3 FR | 7884/948 | 1143/1119 | -6741/171 | 7377/450 | 5/5 | -7372/-445 |
| 4 RF | 7125/630 | 483/471 | -6642/-159 | 6722/226 | 6/6 | -6716/-220 |
| 5 FR | 6160/809 | 569/560 | -5591/-249 | 5622/282 | 6/6 | -5616/-276 |
| 6 RF | 7662/834 | 473/473 | -7189/-361 | 7105/278 | 4/4 | -7101/-274 |

| Stage | F median | R median | Paired R−F | FR R−F | RF R−F |
| --- | --- | --- | --- | --- | --- |
| parent | 7481/821.5 | 489.5/481.5 | -6784/-237 | -6741/-225 | -7189/-361 |
| timeout_setup | 7.5/7.5 | 8/8 | 0/0 | 2/2 | -1/-1 |
| begin_immediate | 3/3 | 3/3 | 0/0 | 1/1 | 0/0 |
| recency | 25/25 | 29.5/29.5 | 4.5/4.5 | 13/13 | -1/-1 |
| durable | 251.5/252 | 245/242 | 9.5/10 | 33/25 | -49/-50 |
| delete_scopes | 54.5/54.5 | 53.5/53.5 | 1/1 | 2/2 | -13/-13 |
| eviction_candidates | 24.5/24.5 | 29/29.5 | 3/3.5 | 5/5 | 0/0 |
| delete_details | 6.5/6.5 | 7/7 | 0/0 | -1/-1 | 1/1 |
| delete_members | 18/18 | 19.5/20 | 0.5/0.5 | 0/0 | 2/3 |
| delete_comics | 12/12 | 11.5/11.5 | 0/0 | 0/0 | 0/0 |
| reconcile | 18.5/18.5 | 19.5/19.5 | 1/1 | 0/0 | 2/2 |
| recency_upsert | 28/28 | 26.5/26.5 | -2.5/-2.5 | -2/-2 | -5/-5 |
| commit | 6977.5/280 | 5.5/5.5 | -6970.5/-275 | -6840/-276 | -7101/-274 |

All 14 launches committed and restored the timeout. Each saw the existing key and 20 durable comic IDs; all four deletes affected zero rows. The recency UPSERT affected one row on F and zero on R. Phase clocks measure application work, not SQL I/O, fsyncs, window visibility or Kindle latency. The parent includes uninstrumented intervals; do not add arm medians or attribute all remaining time to storage.
