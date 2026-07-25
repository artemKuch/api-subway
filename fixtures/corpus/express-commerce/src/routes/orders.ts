import { Router } from "express";
import { createOrder, listOrders } from "../services/orders";

const router = Router();

router.get("/orders", async (_request, response) => {
  response.json(await listOrders());
});

router.post("/orders", async (request, response) => {
  response.status(201).json(await createOrder(request.body));
});

router.get("/orders/:orderId", async (request, response) => {
  const orders = await listOrders();
  response.json(orders.find((order) => order.id === request.params.orderId));
});

export default router;
