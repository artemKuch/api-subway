from fastapi import FastAPI

from operations.routes.orders import router as orders_router

app = FastAPI(dependencies=[])
app.include_router(orders_router, prefix="/api/v1")


@app.get("/health")
async def health():
    return {"ok": True}
