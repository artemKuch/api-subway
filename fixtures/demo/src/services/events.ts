import { readEvents } from '../repositories/events';

export async function listEvents() {
  return readEvents();
}
