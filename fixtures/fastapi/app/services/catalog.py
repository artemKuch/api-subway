import httpx
import stripe

client = stripe.StripeClient()


async def sync_catalog(payload):
    def unused_http_path():
        client = httpx.Client()
        return client.get("https://example.invalid")

    client.Customer.list()
    return {
        "id": "00000000-0000-4000-8000-000000000001",
        **payload.model_dump(),
    }
