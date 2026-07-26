import Redis from 'ioredis';

const redis = new Redis();

export async function readEvents() {
  return redis.lrange('events', 0, 20);
}
