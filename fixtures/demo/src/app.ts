import express from 'express';
import { audit } from './middleware/audit';
import { authorize } from './middleware/auth';
import billingRouter from './routes/billing';
import eventsRouter from './routes/events';
import ordersRouter from './routes/orders';
import usersRouter from './routes/users';

const app = express();

app.use(express.json());
app.use(audit);
app.use('/users', authorize, usersRouter);
app.use('/orders', authorize, ordersRouter);
app.use('/billing', authorize, billingRouter);
app.use('/events', eventsRouter);
app.get('/health', (_request, response) => response.json({ status: 'ok' }));

export default app;
