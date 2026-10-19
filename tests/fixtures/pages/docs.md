# Consumer lag

Lag is the difference, in messages, between the newest offset in a partition and the offset a consumer group has committed.

## Checking lag

Run the group command, then read the `LAG` column, which is reported per partition:

```bash
kafka-consumer-groups --describe --group ledger-events
```

## Settings that matter

| Setting | Default | Effect |
| --- | --- | --- |
| `max.poll.records` | 500 | Records returned by one poll |
| `fetch.max.bytes` | 52428800 | Bytes returned by one fetch |

See [configuration](https://docs.example.com/consumer/config.html#poll) for the rest, including the timeouts.
