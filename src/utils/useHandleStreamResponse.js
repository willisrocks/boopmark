import React, { useEffect, useCallback } from 'react';

function useHandleStreamResponse({ onChunk, onFinish }) {
	const handleStreamResponse = React.useCallback(
		async (response) => {
			if (response.body) {
				const reader = response.body.getReader();
				if (reader) {
					const decoder = new TextDecoder();
					let content = '';
					while (true) {
						const { done, value } = await reader.read();
						if (done) {
							onFinish(content);
							break;
						}
						const chunk = decoder.decode(value, { stream: true });
						content += chunk;
						onChunk(content);
					}
				}
			}
		},
		[onChunk, onFinish]
	);
	const handleStreamResponseRef = React.useRef(handleStreamResponse);
	useEffect(() => {
		handleStreamResponseRef.current = handleStreamResponse;
	}, [handleStreamResponse]);
	return useCallback(
		(response) => handleStreamResponseRef.current(response),
		[]
	);
}

export default useHandleStreamResponse;
