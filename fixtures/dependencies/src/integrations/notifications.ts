import twilio from 'twilio';

const client = twilio('fixture-account', 'fixture-token');

export async function sendReceipt(order: unknown) {
  await client.messages.create({ body: String(order) });
}
