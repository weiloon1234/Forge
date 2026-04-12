use std::time::Duration;

use crate::foundation::{Error, Result};
use crate::redis::namespaced_value;
use crate::support::runtime::{
    LeasedJobToken, MemoryRuntime, RedisRuntime, RuntimeBackend, ScheduledJobToken,
};
use crate::support::QueueId;

const CLAIM_JOB_SCRIPT: &str = r#"
local token = redis.call('LPOP', KEYS[1])
if not token then
  return nil
end
local payload = redis.call('HGET', KEYS[2], token)
if not payload then
  return nil
end
redis.call('ZADD', KEYS[3], ARGV[1], token)
return {token, payload}
"#;

const PROMOTE_DUE_SCRIPT: &str = r#"
local tokens = redis.call('ZRANGEBYSCORE', KEYS[1], '-inf', ARGV[1], 'LIMIT', 0, ARGV[2])
if #tokens == 0 then
  return 0
end
redis.call('ZREM', KEYS[1], unpack(tokens))
redis.call('RPUSH', KEYS[2], unpack(tokens))
return #tokens
"#;

const REQUEUE_EXPIRED_SCRIPT: &str = r#"
local tokens = redis.call('ZRANGEBYSCORE', KEYS[1], '-inf', ARGV[1], 'LIMIT', 0, ARGV[2])
if #tokens == 0 then
  return 0
end
redis.call('ZREM', KEYS[1], unpack(tokens))
redis.call('RPUSH', KEYS[2], unpack(tokens))
return #tokens
"#;

const RENEW_LEASE_SCRIPT: &str = r#"
if redis.call('ZSCORE', KEYS[1], ARGV[1]) then
  redis.call('ZADD', KEYS[1], ARGV[2], ARGV[1])
  return 1
end
return 0
"#;

const ACK_JOB_SCRIPT: &str = r#"
if redis.call('ZREM', KEYS[1], ARGV[1]) == 0 then
  return 0
end
redis.call('HDEL', KEYS[2], ARGV[1])
return 1
"#;

const RETRY_JOB_SCRIPT: &str = r#"
if redis.call('ZREM', KEYS[1], ARGV[1]) == 0 then
  return 0
end
redis.call('HDEL', KEYS[2], ARGV[1])
redis.call('HSET', KEYS[2], ARGV[2], ARGV[3])
redis.call('ZADD', KEYS[3], ARGV[4], ARGV[2])
return 1
"#;

const DEAD_LETTER_JOB_SCRIPT: &str = r#"
if redis.call('ZREM', KEYS[1], ARGV[1]) == 0 then
  return 0
end
redis.call('HDEL', KEYS[2], ARGV[1])
redis.call('RPUSH', KEYS[3], ARGV[2])
return 1
"#;

#[derive(Clone, Debug)]
pub(crate) struct ClaimedJobLease {
    pub queue: QueueId,
    pub token: String,
    pub payload: String,
}

impl RuntimeBackend {
    pub(crate) async fn enqueue_job(
        &self,
        queue: &QueueId,
        token: &str,
        payload: &str,
    ) -> Result<()> {
        match self {
            Self::Redis(runtime) => enqueue_job_redis(runtime, queue, token, payload).await,
            Self::Memory(runtime) => enqueue_job_memory(runtime, queue, token, payload).await,
        }
    }

    pub(crate) async fn schedule_job(
        &self,
        queue: &QueueId,
        token: &str,
        payload: &str,
        run_at_millis: i64,
    ) -> Result<()> {
        match self {
            Self::Redis(runtime) => {
                schedule_job_redis(runtime, queue, token, payload, run_at_millis).await
            }
            Self::Memory(runtime) => {
                schedule_job_memory(runtime, queue, token, payload, run_at_millis).await
            }
        }
    }

    pub(crate) async fn promote_due_jobs(
        &self,
        queues: &[QueueId],
        now_millis: i64,
        limit: usize,
    ) -> Result<usize> {
        match self {
            Self::Redis(runtime) => {
                promote_due_jobs_redis(runtime, queues, now_millis, limit).await
            }
            Self::Memory(runtime) => {
                promote_due_jobs_memory(runtime, queues, now_millis, limit).await
            }
        }
    }

    pub(crate) async fn requeue_expired_jobs(
        &self,
        queues: &[QueueId],
        now_millis: i64,
        limit: usize,
    ) -> Result<usize> {
        match self {
            Self::Redis(runtime) => {
                requeue_expired_jobs_redis(runtime, queues, now_millis, limit).await
            }
            Self::Memory(runtime) => {
                requeue_expired_jobs_memory(runtime, queues, now_millis, limit).await
            }
        }
    }

    pub(crate) async fn claim_job(
        &self,
        queues: &[QueueId],
        lease_ttl: Duration,
    ) -> Result<Option<ClaimedJobLease>> {
        match self {
            Self::Redis(runtime) => claim_job_redis(runtime, queues, lease_ttl).await,
            Self::Memory(runtime) => claim_job_memory(runtime, queues, lease_ttl).await,
        }
    }

    pub(crate) async fn renew_job_lease(
        &self,
        queue: &QueueId,
        token: &str,
        lease_ttl: Duration,
    ) -> Result<bool> {
        match self {
            Self::Redis(runtime) => renew_job_lease_redis(runtime, queue, token, lease_ttl).await,
            Self::Memory(runtime) => renew_job_lease_memory(runtime, queue, token, lease_ttl).await,
        }
    }

    pub(crate) async fn ack_job(&self, queue: &QueueId, token: &str) -> Result<bool> {
        match self {
            Self::Redis(runtime) => ack_job_redis(runtime, queue, token).await,
            Self::Memory(runtime) => ack_job_memory(runtime, queue, token).await,
        }
    }

    pub(crate) async fn retry_job(
        &self,
        queue: &QueueId,
        token: &str,
        new_token: &str,
        payload: &str,
        run_at_millis: i64,
    ) -> Result<bool> {
        match self {
            Self::Redis(runtime) => {
                retry_job_redis(runtime, queue, token, new_token, payload, run_at_millis).await
            }
            Self::Memory(runtime) => {
                retry_job_memory(runtime, queue, token, new_token, payload, run_at_millis).await
            }
        }
    }

    pub(crate) async fn dead_letter_job(
        &self,
        queue: &QueueId,
        token: &str,
        payload: &str,
    ) -> Result<bool> {
        match self {
            Self::Redis(runtime) => dead_letter_job_redis(runtime, queue, token, payload).await,
            Self::Memory(runtime) => dead_letter_job_memory(runtime, queue, token, payload).await,
        }
    }

    /// Create batch metadata. Returns nothing; the batch_id is caller-generated.
    pub(crate) async fn create_batch(
        &self,
        batch_id: &str,
        total: u64,
        on_complete_payload: Option<&str>,
        on_complete_queue: Option<&str>,
    ) -> Result<()> {
        match self {
            Self::Redis(runtime) => {
                create_batch_redis(runtime, batch_id, total, on_complete_payload, on_complete_queue)
                    .await
            }
            Self::Memory(runtime) => {
                create_batch_memory(runtime, batch_id, total, on_complete_payload, on_complete_queue)
                    .await
            }
        }
    }

    /// Increment completed count for a batch. Returns `(completed, total, on_complete_payload, on_complete_queue)`.
    pub(crate) async fn increment_batch_completed(
        &self,
        batch_id: &str,
    ) -> Result<(u64, u64, Option<String>, Option<String>)> {
        match self {
            Self::Redis(runtime) => increment_batch_completed_redis(runtime, batch_id).await,
            Self::Memory(runtime) => increment_batch_completed_memory(runtime, batch_id).await,
        }
    }

    #[cfg(test)]
    pub(crate) async fn dead_letters(&self, queue: &QueueId) -> Result<Vec<String>> {
        match self {
            Self::Redis(_) => Ok(Vec::new()),
            Self::Memory(runtime) => dead_letters_memory(runtime, queue).await,
        }
    }
}

fn ready_key(runtime: &RedisRuntime, queue: &QueueId) -> String {
    namespaced_value(&runtime.namespace, &format!("jobs:ready:{queue}"))
}

fn scheduled_key(runtime: &RedisRuntime, queue: &QueueId) -> String {
    namespaced_value(&runtime.namespace, &format!("jobs:scheduled:{queue}"))
}

fn leased_key(runtime: &RedisRuntime, queue: &QueueId) -> String {
    namespaced_value(&runtime.namespace, &format!("jobs:leased:{queue}"))
}

fn payload_key(runtime: &RedisRuntime, queue: &QueueId) -> String {
    namespaced_value(&runtime.namespace, &format!("jobs:payload:{queue}"))
}

fn dead_letter_key(runtime: &RedisRuntime, queue: &QueueId) -> String {
    namespaced_value(&runtime.namespace, &format!("jobs:dead:{queue}"))
}

fn expires_at(lease_ttl: Duration) -> i64 {
    chrono::Utc::now().timestamp_millis() + lease_ttl.as_millis() as i64
}

async fn enqueue_job_redis(
    runtime: &RedisRuntime,
    queue: &QueueId,
    token: &str,
    payload: &str,
) -> Result<()> {
    let mut conn = runtime
        .client
        .get_multiplexed_async_connection()
        .await
        .map_err(Error::other)?;
    let _: () = ::redis::pipe()
        .atomic()
        .cmd("HSET")
        .arg(payload_key(runtime, queue))
        .arg(token)
        .arg(payload)
        .ignore()
        .cmd("RPUSH")
        .arg(ready_key(runtime, queue))
        .arg(token)
        .ignore()
        .query_async(&mut conn)
        .await
        .map_err(Error::other)?;
    Ok(())
}

async fn schedule_job_redis(
    runtime: &RedisRuntime,
    queue: &QueueId,
    token: &str,
    payload: &str,
    run_at_millis: i64,
) -> Result<()> {
    let mut conn = runtime
        .client
        .get_multiplexed_async_connection()
        .await
        .map_err(Error::other)?;
    let _: () = ::redis::pipe()
        .atomic()
        .cmd("HSET")
        .arg(payload_key(runtime, queue))
        .arg(token)
        .arg(payload)
        .ignore()
        .cmd("ZADD")
        .arg(scheduled_key(runtime, queue))
        .arg(run_at_millis)
        .arg(token)
        .ignore()
        .query_async(&mut conn)
        .await
        .map_err(Error::other)?;
    Ok(())
}

async fn promote_due_jobs_redis(
    runtime: &RedisRuntime,
    queues: &[QueueId],
    now_millis: i64,
    limit: usize,
) -> Result<usize> {
    let mut moved = 0usize;
    for queue in queues {
        if moved >= limit {
            break;
        }

        let mut conn = runtime
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(Error::other)?;
        let count: i64 = ::redis::cmd("EVAL")
            .arg(PROMOTE_DUE_SCRIPT)
            .arg(2)
            .arg(scheduled_key(runtime, queue))
            .arg(ready_key(runtime, queue))
            .arg(now_millis)
            .arg((limit - moved) as i64)
            .query_async(&mut conn)
            .await
            .map_err(Error::other)?;
        moved += count.max(0) as usize;
    }

    Ok(moved)
}

async fn requeue_expired_jobs_redis(
    runtime: &RedisRuntime,
    queues: &[QueueId],
    now_millis: i64,
    limit: usize,
) -> Result<usize> {
    let mut moved = 0usize;
    for queue in queues {
        if moved >= limit {
            break;
        }

        let mut conn = runtime
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(Error::other)?;
        let count: i64 = ::redis::cmd("EVAL")
            .arg(REQUEUE_EXPIRED_SCRIPT)
            .arg(2)
            .arg(leased_key(runtime, queue))
            .arg(ready_key(runtime, queue))
            .arg(now_millis)
            .arg((limit - moved) as i64)
            .query_async(&mut conn)
            .await
            .map_err(Error::other)?;
        moved += count.max(0) as usize;
    }

    Ok(moved)
}

async fn claim_job_redis(
    runtime: &RedisRuntime,
    queues: &[QueueId],
    lease_ttl: Duration,
) -> Result<Option<ClaimedJobLease>> {
    let lease_expires_at = expires_at(lease_ttl);
    for queue in queues {
        let mut conn = runtime
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(Error::other)?;
        let result: Option<Vec<String>> = ::redis::cmd("EVAL")
            .arg(CLAIM_JOB_SCRIPT)
            .arg(3)
            .arg(ready_key(runtime, queue))
            .arg(payload_key(runtime, queue))
            .arg(leased_key(runtime, queue))
            .arg(lease_expires_at)
            .query_async(&mut conn)
            .await
            .map_err(Error::other)?;

        if let Some(values) = result {
            if values.len() == 2 {
                return Ok(Some(ClaimedJobLease {
                    queue: queue.clone(),
                    token: values[0].clone(),
                    payload: values[1].clone(),
                }));
            }
        }
    }

    Ok(None)
}

async fn renew_job_lease_redis(
    runtime: &RedisRuntime,
    queue: &QueueId,
    token: &str,
    lease_ttl: Duration,
) -> Result<bool> {
    let mut conn = runtime
        .client
        .get_multiplexed_async_connection()
        .await
        .map_err(Error::other)?;
    let renewed: i64 = ::redis::cmd("EVAL")
        .arg(RENEW_LEASE_SCRIPT)
        .arg(1)
        .arg(leased_key(runtime, queue))
        .arg(token)
        .arg(expires_at(lease_ttl))
        .query_async(&mut conn)
        .await
        .map_err(Error::other)?;
    Ok(renewed == 1)
}

async fn ack_job_redis(runtime: &RedisRuntime, queue: &QueueId, token: &str) -> Result<bool> {
    let mut conn = runtime
        .client
        .get_multiplexed_async_connection()
        .await
        .map_err(Error::other)?;
    let acknowledged: i64 = ::redis::cmd("EVAL")
        .arg(ACK_JOB_SCRIPT)
        .arg(2)
        .arg(leased_key(runtime, queue))
        .arg(payload_key(runtime, queue))
        .arg(token)
        .query_async(&mut conn)
        .await
        .map_err(Error::other)?;
    Ok(acknowledged == 1)
}

async fn retry_job_redis(
    runtime: &RedisRuntime,
    queue: &QueueId,
    token: &str,
    new_token: &str,
    payload: &str,
    run_at_millis: i64,
) -> Result<bool> {
    let mut conn = runtime
        .client
        .get_multiplexed_async_connection()
        .await
        .map_err(Error::other)?;
    let rescheduled: i64 = ::redis::cmd("EVAL")
        .arg(RETRY_JOB_SCRIPT)
        .arg(3)
        .arg(leased_key(runtime, queue))
        .arg(payload_key(runtime, queue))
        .arg(scheduled_key(runtime, queue))
        .arg(token)
        .arg(new_token)
        .arg(payload)
        .arg(run_at_millis)
        .query_async(&mut conn)
        .await
        .map_err(Error::other)?;
    Ok(rescheduled == 1)
}

async fn dead_letter_job_redis(
    runtime: &RedisRuntime,
    queue: &QueueId,
    token: &str,
    payload: &str,
) -> Result<bool> {
    let mut conn = runtime
        .client
        .get_multiplexed_async_connection()
        .await
        .map_err(Error::other)?;
    let dead_lettered: i64 = ::redis::cmd("EVAL")
        .arg(DEAD_LETTER_JOB_SCRIPT)
        .arg(3)
        .arg(leased_key(runtime, queue))
        .arg(payload_key(runtime, queue))
        .arg(dead_letter_key(runtime, queue))
        .arg(token)
        .arg(payload)
        .query_async(&mut conn)
        .await
        .map_err(Error::other)?;
    Ok(dead_lettered == 1)
}

async fn enqueue_job_memory(
    runtime: &MemoryRuntime,
    queue: &QueueId,
    token: &str,
    payload: &str,
) -> Result<()> {
    let mut payloads = runtime.payloads.lock().await;
    payloads.insert(token.to_string(), payload.to_string());
    drop(payloads);

    let mut ready = runtime.ready_queues.lock().await;
    ready
        .entry(queue.clone())
        .or_default()
        .push_back(token.to_string());
    drop(ready);
    runtime.notify.notify_waiters();
    Ok(())
}

async fn schedule_job_memory(
    runtime: &MemoryRuntime,
    queue: &QueueId,
    token: &str,
    payload: &str,
    run_at_millis: i64,
) -> Result<()> {
    let mut payloads = runtime.payloads.lock().await;
    payloads.insert(token.to_string(), payload.to_string());
    drop(payloads);

    let mut scheduled = runtime.scheduled_jobs.lock().await;
    scheduled
        .entry(queue.clone())
        .or_default()
        .push(ScheduledJobToken {
            run_at_millis,
            token: token.to_string(),
        });
    if let Some(items) = scheduled.get_mut(queue) {
        items.sort_by_key(|item| item.run_at_millis);
    }
    drop(scheduled);
    runtime.notify.notify_waiters();
    Ok(())
}

async fn promote_due_jobs_memory(
    runtime: &MemoryRuntime,
    queues: &[QueueId],
    now_millis: i64,
    limit: usize,
) -> Result<usize> {
    let mut moved = 0usize;
    let mut scheduled = runtime.scheduled_jobs.lock().await;
    let mut ready = runtime.ready_queues.lock().await;
    for queue in queues {
        if moved >= limit {
            break;
        }
        let mut due = Vec::new();
        let mut pending = Vec::new();
        for item in scheduled.remove(queue).unwrap_or_default() {
            if item.run_at_millis <= now_millis && moved + due.len() < limit {
                due.push(item.token);
            } else {
                pending.push(item);
            }
        }
        if !pending.is_empty() {
            scheduled.insert(queue.clone(), pending);
        }
        if !due.is_empty() {
            let queue_items = ready.entry(queue.clone()).or_default();
            for token in due {
                queue_items.push_back(token);
                moved += 1;
            }
        }
    }
    drop(ready);
    drop(scheduled);
    if moved > 0 {
        runtime.notify.notify_waiters();
    }
    Ok(moved)
}

async fn requeue_expired_jobs_memory(
    runtime: &MemoryRuntime,
    queues: &[QueueId],
    now_millis: i64,
    limit: usize,
) -> Result<usize> {
    let mut moved = 0usize;
    let mut leased = runtime.leased_jobs.lock().await;
    let mut ready = runtime.ready_queues.lock().await;
    for queue in queues {
        if moved >= limit {
            break;
        }
        let mut expired = Vec::new();
        let mut active = Vec::new();
        for item in leased.remove(queue).unwrap_or_default() {
            if item.expires_at_millis <= now_millis && moved + expired.len() < limit {
                expired.push(item.token);
            } else {
                active.push(item);
            }
        }
        if !active.is_empty() {
            leased.insert(queue.clone(), active);
        }
        if !expired.is_empty() {
            let queue_items = ready.entry(queue.clone()).or_default();
            for token in expired {
                queue_items.push_back(token);
                moved += 1;
            }
        }
    }
    drop(ready);
    drop(leased);
    if moved > 0 {
        runtime.notify.notify_waiters();
    }
    Ok(moved)
}

async fn claim_job_memory(
    runtime: &MemoryRuntime,
    queues: &[QueueId],
    lease_ttl: Duration,
) -> Result<Option<ClaimedJobLease>> {
    let mut ready = runtime.ready_queues.lock().await;
    let mut found = None;
    for queue in queues {
        if let Some(items) = ready.get_mut(queue) {
            if let Some(token) = items.pop_front() {
                found = Some((queue.clone(), token));
                break;
            }
        }
    }
    drop(ready);

    let Some((queue, token)) = found else {
        return Ok(None);
    };

    let payload = runtime
        .payloads
        .lock()
        .await
        .get(&token)
        .cloned()
        .ok_or_else(|| Error::message("job payload missing from memory runtime"))?;

    let mut leased = runtime.leased_jobs.lock().await;
    leased
        .entry(queue.clone())
        .or_default()
        .push(LeasedJobToken {
            expires_at_millis: expires_at(lease_ttl),
            token: token.clone(),
        });
    drop(leased);

    Ok(Some(ClaimedJobLease {
        queue,
        token,
        payload,
    }))
}

async fn renew_job_lease_memory(
    runtime: &MemoryRuntime,
    queue: &QueueId,
    token: &str,
    lease_ttl: Duration,
) -> Result<bool> {
    let mut leased = runtime.leased_jobs.lock().await;
    if let Some(items) = leased.get_mut(queue) {
        for item in items {
            if item.token == token {
                item.expires_at_millis = expires_at(lease_ttl);
                return Ok(true);
            }
        }
    }
    Ok(false)
}

async fn ack_job_memory(runtime: &MemoryRuntime, queue: &QueueId, token: &str) -> Result<bool> {
    let mut leased = runtime.leased_jobs.lock().await;
    let mut removed = false;
    if let Some(items) = leased.get_mut(queue) {
        let before = items.len();
        items.retain(|item| item.token != token);
        removed = items.len() != before;
    }
    drop(leased);
    if removed {
        runtime.payloads.lock().await.remove(token);
    }
    Ok(removed)
}

async fn retry_job_memory(
    runtime: &MemoryRuntime,
    queue: &QueueId,
    token: &str,
    new_token: &str,
    payload: &str,
    run_at_millis: i64,
) -> Result<bool> {
    let removed = ack_like_memory(runtime, queue, token).await?;
    if !removed {
        return Ok(false);
    }

    let mut payloads = runtime.payloads.lock().await;
    payloads.insert(new_token.to_string(), payload.to_string());
    drop(payloads);

    let mut scheduled = runtime.scheduled_jobs.lock().await;
    scheduled
        .entry(queue.clone())
        .or_default()
        .push(ScheduledJobToken {
            run_at_millis,
            token: new_token.to_string(),
        });
    if let Some(items) = scheduled.get_mut(queue) {
        items.sort_by_key(|item| item.run_at_millis);
    }
    drop(scheduled);
    runtime.notify.notify_waiters();
    Ok(true)
}

async fn dead_letter_job_memory(
    runtime: &MemoryRuntime,
    queue: &QueueId,
    token: &str,
    payload: &str,
) -> Result<bool> {
    let removed = ack_like_memory(runtime, queue, token).await?;
    if !removed {
        return Ok(false);
    }

    let mut dead_letters = runtime.dead_letters.lock().await;
    dead_letters
        .entry(queue.clone())
        .or_default()
        .push(payload.to_string());
    Ok(true)
}

async fn ack_like_memory(runtime: &MemoryRuntime, queue: &QueueId, token: &str) -> Result<bool> {
    let mut leased = runtime.leased_jobs.lock().await;
    let mut removed = false;
    if let Some(items) = leased.get_mut(queue) {
        let before = items.len();
        items.retain(|item| item.token != token);
        removed = items.len() != before;
    }
    drop(leased);
    if removed {
        runtime.payloads.lock().await.remove(token);
    }
    Ok(removed)
}

// ---------------------------------------------------------------------------
// Batch tracking — Redis
// ---------------------------------------------------------------------------

fn batch_key(runtime: &RedisRuntime, batch_id: &str) -> String {
    namespaced_value(&runtime.namespace, &format!("jobs:batch:{batch_id}"))
}

const CREATE_BATCH_SCRIPT: &str = r#"
redis.call('HSET', KEYS[1], 'total', ARGV[1], 'completed', '0')
if ARGV[2] ~= '' then
  redis.call('HSET', KEYS[1], 'on_complete_payload', ARGV[2])
end
if ARGV[3] ~= '' then
  redis.call('HSET', KEYS[1], 'on_complete_queue', ARGV[3])
end
redis.call('EXPIRE', KEYS[1], 86400)
return 1
"#;

const INCREMENT_BATCH_SCRIPT: &str = r#"
local completed = redis.call('HINCRBY', KEYS[1], 'completed', 1)
local total = tonumber(redis.call('HGET', KEYS[1], 'total'))
local payload = redis.call('HGET', KEYS[1], 'on_complete_payload') or ''
local queue = redis.call('HGET', KEYS[1], 'on_complete_queue') or ''
return {completed, total, payload, queue}
"#;

async fn create_batch_redis(
    runtime: &RedisRuntime,
    batch_id: &str,
    total: u64,
    on_complete_payload: Option<&str>,
    on_complete_queue: Option<&str>,
) -> Result<()> {
    let mut conn = runtime
        .client
        .get_multiplexed_async_connection()
        .await
        .map_err(Error::other)?;
    let _: i64 = ::redis::cmd("EVAL")
        .arg(CREATE_BATCH_SCRIPT)
        .arg(1)
        .arg(batch_key(runtime, batch_id))
        .arg(total)
        .arg(on_complete_payload.unwrap_or(""))
        .arg(on_complete_queue.unwrap_or(""))
        .query_async(&mut conn)
        .await
        .map_err(Error::other)?;
    Ok(())
}

async fn increment_batch_completed_redis(
    runtime: &RedisRuntime,
    batch_id: &str,
) -> Result<(u64, u64, Option<String>, Option<String>)> {
    let mut conn = runtime
        .client
        .get_multiplexed_async_connection()
        .await
        .map_err(Error::other)?;
    let result: Vec<String> = ::redis::cmd("EVAL")
        .arg(INCREMENT_BATCH_SCRIPT)
        .arg(1)
        .arg(batch_key(runtime, batch_id))
        .query_async(&mut conn)
        .await
        .map_err(Error::other)?;

    let completed = result
        .first()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let total = result
        .get(1)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let payload = result.get(2).filter(|s| !s.is_empty()).cloned();
    let queue = result.get(3).filter(|s| !s.is_empty()).cloned();
    Ok((completed, total, payload, queue))
}

// ---------------------------------------------------------------------------
// Batch tracking — Memory
// ---------------------------------------------------------------------------

async fn create_batch_memory(
    runtime: &MemoryRuntime,
    batch_id: &str,
    total: u64,
    on_complete_payload: Option<&str>,
    on_complete_queue: Option<&str>,
) -> Result<()> {
    use crate::support::runtime::MemoryBatchMeta;
    let mut batches = runtime.batches.lock().await;
    batches.insert(
        batch_id.to_string(),
        MemoryBatchMeta {
            total,
            completed: 0,
            on_complete_job: on_complete_payload.map(|s| s.to_string()),
            on_complete_queue: on_complete_queue.map(|s| s.to_string()),
        },
    );
    Ok(())
}

async fn increment_batch_completed_memory(
    runtime: &MemoryRuntime,
    batch_id: &str,
) -> Result<(u64, u64, Option<String>, Option<String>)> {
    let mut batches = runtime.batches.lock().await;
    let meta = batches
        .get_mut(batch_id)
        .ok_or_else(|| Error::message(format!("batch `{batch_id}` not found")))?;
    meta.completed += 1;
    let result = (
        meta.completed,
        meta.total,
        meta.on_complete_job.clone(),
        meta.on_complete_queue.clone(),
    );
    Ok(result)
}

#[cfg(test)]
async fn dead_letters_memory(runtime: &MemoryRuntime, queue: &QueueId) -> Result<Vec<String>> {
    let dead_letters = runtime.dead_letters.lock().await;
    Ok(dead_letters.get(queue).cloned().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::RuntimeBackend;
    use crate::support::QueueId;

    #[tokio::test]
    async fn memory_backend_claims_and_acks_leased_jobs() {
        let backend = RuntimeBackend::memory("job-backend-ack");
        let queue = QueueId::new("default");
        backend
            .enqueue_job(&queue, "token-1", "{\"job\":\"ok\"}")
            .await
            .unwrap();

        let claimed = backend
            .claim_job(std::slice::from_ref(&queue), Duration::from_millis(50))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed.queue, queue);
        assert_eq!(claimed.token, "token-1");

        assert!(backend.ack_job(&queue, "token-1").await.unwrap());
        assert!(backend
            .claim_job(std::slice::from_ref(&queue), Duration::from_millis(50))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn memory_backend_requeues_expired_leases() {
        let backend = RuntimeBackend::memory("job-backend-requeue");
        let queue = QueueId::new("default");
        backend
            .enqueue_job(&queue, "token-1", "{\"job\":\"recover\"}")
            .await
            .unwrap();

        let claimed = backend
            .claim_job(std::slice::from_ref(&queue), Duration::from_millis(5))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed.token, "token-1");

        tokio::time::sleep(Duration::from_millis(10)).await;
        let requeued = backend
            .requeue_expired_jobs(
                std::slice::from_ref(&queue),
                chrono::Utc::now().timestamp_millis(),
                8,
            )
            .await
            .unwrap();
        assert_eq!(requeued, 1);

        let reclaimed = backend
            .claim_job(std::slice::from_ref(&queue), Duration::from_millis(50))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reclaimed.token, "token-1");
    }
}
