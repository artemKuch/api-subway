import express from 'express';
import { audit } from './middleware/audit';
import usersRouter from './routes/users';

const app = express();

app.use(express.json());
app.use('/api', audit, usersRouter);

app.get('/health', (_request, response) => response.json({ ok: true }));

export default app;
