import { Client } from 'pg';

const database = new Client();

export async function findOrders() {
  return database.query('select id from orders');
}

export async function insertOrder(input: unknown) {
  await database.query('insert into orders values ($1)', [input]);
  return input;
}

export async function replaceOrder(id: string, input: unknown) {
  await database.query('update orders set payload = $1 where id = $2', [input, id]);
  return input;
}
