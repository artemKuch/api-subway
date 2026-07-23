import { Router } from 'express';
import Stripe from 'stripe';
import * as orderOperations from '../services/orders';

const router = Router();

router.get('/orders', async (_request, response) => {
  response.json(await orderOperations.listOrders());
});

router.post('/orders', async (request, response) => {
  response.status(201).json(await orderOperations.createOrder(request.body));
});

router.patch('/orders/:id', async (request, response) => {
  const operation = request.query.operation;
  response.json(await orderOperations[operation](request.params.id));
});

router.delete('/orders/:id', (_request, response) => {
  response.status(204).end();
});

export default router;
