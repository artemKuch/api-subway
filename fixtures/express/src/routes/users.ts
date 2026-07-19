import { Router } from 'express';
import Stripe from 'stripe';
import { authorize, validateUser } from '../middleware/auth';
import { UserInput, UserOutput } from '../schemas/users';

const router = Router();
const stripe = new Stripe('local-fixture-key');

router.use(authorize);
router.get('/users', validateUser, async (_request, response) => {
  await stripe.customers.list();
  response.json([]);
});
router.route('/users/:id').get(validateUser, showUser).patch(validateUser, updateUser);

function showUser() {}
function updateUser(request, response) {
  const input = UserInput.parse(request.body);
  response.status(200).json(UserOutput.parse({ id: request.params.id, ...input }));
}

export default router;
