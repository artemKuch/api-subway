import { Router } from 'express';
import { createOrder, listOrders, updateOrder } from '../services/orders';

const router = Router();

router.get('/', async (_request, response) => {
  response.json(await listOrders());
});

router.post('/', async (request, response) => {
  response.status(201).json(await createOrder(request.body));
});

router.put('/:id', async (request, response) => {
  response.json(await updateOrder(request.params.id, request.body));
});

export default router;
