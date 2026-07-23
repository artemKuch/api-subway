import express from 'express';
import ordersRouter from './routes/orders';

const app = express();

app.use('/api', ordersRouter);

export default app;
