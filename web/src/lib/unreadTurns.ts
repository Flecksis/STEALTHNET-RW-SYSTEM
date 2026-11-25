export type UnreadTurns = {
  put: (tickets: number) => void;
  ask: (request: () => Promise<number>) => Promise<void>;
  drop: () => void;
};

export function unreadTurns(show: (tickets: number) => void): UnreadTurns {
  let turn = 0;
  return {
    put: (tickets) => {
      turn += 1;
      show(tickets);
    },
    ask: async (request) => {
      const asked = turn;
      try {
        const tickets = await request();
        if (asked === turn) {
          show(tickets);
        }
      } catch {
        return;
      }
    },
    drop: () => {
      turn += 1;
    },
  };
}
