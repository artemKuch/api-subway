import { loadOrders, saveOrder } from "../repositories/orders";
import { sendOrderReceipt } from "../integrations/notifications";

export async function listOrders() {
  return loadOrders();
}

export async function createOrder(input: unknown) {
  const order = await saveOrder(input);
  await sendOrderReceipt(order);
  return order;
}
