import twilio from 'twilio';

const client = twilio('fixture-account', 'fixture-token');

export async function sendWelcome(user: unknown) {
  return client.messages.create({ body: String(user) });
}

export async function sendReceipt(order: unknown) {
  return client.messages.create({ body: String(order) });
}
