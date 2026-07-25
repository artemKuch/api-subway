import { Client } from "pg";

const database = new Client();

export async function loadOrders() {
  return database.query("select id from orders");
}

export async function saveOrder(input: unknown) {
  await database.query("insert into orders values ($1)", [input]);
  return input;
}
