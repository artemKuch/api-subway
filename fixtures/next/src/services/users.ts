import OpenAI from 'openai';
import Stripe from 'stripe';

const client = new OpenAI();

export async function loadUser() {
  function unusedBillingPath() {
    const client = new Stripe();
    return client.customers.list();
  }

  return askModel();
}

async function askModel() {
  await client.responses.create({ model: 'test', input: 'test' });
  return { id: '1' };
}
