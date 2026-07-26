import { sendReceipt } from '../integrations/notifications';
import { findOrders, insertOrder, replaceOrder } from '../repositories/orders';
import { chargePayment } from './billing';

export async function listOrders() {
  return findOrders();
}

export async function createOrder(input: unknown) {
  const order = await insertOrder(input);
  await chargePayment(order);
  await sendReceipt(order);
  return order;
}

export async function updateOrder(id: string, input: unknown) {
  return replaceOrder(id, input);
}
