import express from "express";
import { requireSession } from "./middleware/session";
import ordersRouter from "./routes/orders";

const app = express();

app.use(express.json());
app.use("/api/v1", requireSession, ordersRouter);
app.get("/health", (_request, response) => response.json({ ok: true }));

export default app;
