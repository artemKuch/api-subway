import stripe
from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine

engine = create_async_engine("postgresql+asyncpg://local")


async def list_orders():
    async with engine.connect() as connection:
        return await connection.execute(text("select id from orders"))


async def load_order(order_id: str):
    async with engine.connect() as connection:
        return await connection.execute(text("select id from orders where id=:id"))


async def create_order(payload: dict):
    stripe.PaymentIntent.create(amount=1000, currency="usd")
    return payload
