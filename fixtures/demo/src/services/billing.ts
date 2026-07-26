import Stripe from 'stripe';

const stripe = new Stripe('fixture-key');

export async function listInvoices() {
  return stripe.invoices.list();
}

export async function chargePayment(order: unknown) {
  return stripe.paymentIntents.create({ metadata: { order: String(order) } });
}

export async function refundPayment(input: unknown) {
  return stripe.refunds.create({ payment_intent: String(input) });
}
