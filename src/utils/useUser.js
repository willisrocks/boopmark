import { useSession } from '@auth/create/react';
import React from 'react';

export const useUser = () => {
	const { data: session, status } = useSession();
	const id = session?.user?.id;

	const [user, setUser] = React.useState(session?.user ?? null);

	const fetchUser = React.useCallback(async (session) => {
		return session?.user;
	}, []);

	const refetchUser = React.useCallback(() => {
		if (id) {
			fetchUser(session).then(setUser);
		} else {
			setUser(null);
		}
	}, [fetchUser, id]);

	React.useEffect(refetchUser, [refetchUser]);

	return {
		user,
		data: user,
		loading: status === 'loading' || (status === 'authenticated' && !user),
		refetch: refetchUser,
	};
};

export default useUser;
