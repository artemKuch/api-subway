import twilio from "twilio";

const client = twilio("account", "token");

export async function sendOrderReceipt(order: unknown) {
  await client.messages.create({ body: String(order) });
}
