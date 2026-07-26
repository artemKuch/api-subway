import { Router } from 'express';
import { listEvents } from '../services/events';

const router = Router();

router.get('/', async (_request, response) => {
  response.json(await listEvents());
});

export default router;
