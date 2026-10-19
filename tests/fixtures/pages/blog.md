## Tuning batch sizes

Batch size is the knob everyone reaches for first, and for good reason: it trades latency for throughput in one number.

Too small, and every record pays the fixed cost of a request. Too large, and a single batch can hold a partition for seconds, which is exactly when lag starts to climb.

My rule of thumb:

1. Start from the default.
2. Measure p99 latency and lag together, never one alone.
3. Change one thing at a time.

That is all there is to it, most of the time, though the exceptions are where the interesting stories live.
