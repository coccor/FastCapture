# Payments platform hit by overnight lag

By A. Writer, technology desk

A nightly batch job slowed card payments for about half an hour on Saturday, the company said, after a consumer group fell behind its topic.

Engineers traced the delay to the ledger batch, which writes millions of records between two and three in the morning. Lag on the `ledger-events` group peaked at roughly forty thousand messages, then recovered by 02:35.

## What changes next

The team plans to cap the batch size and to alert on lag earlier, according to a [published review](https://www.dailyledger.example/tech/2026/10/postmortem). Customers were not charged twice, and no payments were lost.

> We should have seen this coming from the dashboards.
