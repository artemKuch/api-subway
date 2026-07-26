import { Router } from 'express';
import { createUser, getUser, listUsers } from '../services/users';

const router = Router();

router.get('/', async (_request, response) => {
  response.json(await listUsers());
});

router.post('/', async (request, response) => {
  response.status(201).json(await createUser(request.body));
});

router.get('/:id', async (request, response) => {
  response.json(await getUser(request.params.id));
});

export default router;
