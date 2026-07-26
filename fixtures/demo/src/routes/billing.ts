import { Router } from 'express';
import { listInvoices, refundPayment } from '../services/billing';

const router = Router();

router.get('/invoices', async (_request, response) => {
  response.json(await listInvoices());
});

router.post('/refunds', async (request, response) => {
  response.status(201).json(await refundPayment(request.body));
});

export default router;
