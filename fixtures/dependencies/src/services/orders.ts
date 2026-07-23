import { findOrders, insertOrder } from '../repositories/orders';
import { sendReceipt } from '../integrations/notifications';

export async function listOrders() {
  return findOrders();
}

export async function createOrder(input: unknown) {
  const order = await insertOrder(input);
  await sendReceipt(order);
  return order;
}

export async function cancelOrder(id: string) {
  return { id, cancelled: true };
}
