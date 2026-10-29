--!strict
-- Tests for scatter.luau: its stones lie within its radius, the same seed gives the same
-- scatter, and Bake keeps the stones once the scatter is baked.
local function scatter(props: any): any
	return {
		name = "Test scatter",
		components = {
			Transform = { translation = { -20, 0, 20 } },
			Script = { path = "examples/scatter.luau", props = props },
		},
	}
end

local function stones(e: Entity): { Entity }
	local out = {}
	for _, child in e:children() do
		if string.find(child.name or "", "^Stone %d") then
			table.insert(out, child)
		end
	end
	return out
end

describe("Scatter", function()
	it("lays its stones within its radius", function(t)
		local e = t:spawn(scatter({ count = 12, radius = 2 }))
		local all = stones(e)
		expect(#all).to_be(12)
		for _, s in all do
			expect(s.position:flat():length() <= 2).to_be(true)
		end
	end)

	it("makes the same scatter from the same seed", function(t)
		local a = stones(t:spawn(scatter({ count = 5, seed = 7 })))
		local b = stones(t:spawn(scatter({ count = 5, seed = 7 })))
		for i = 1, 5 do
			expect(a[i].position).to_be_near(b[i].position, 1e-6)
		end
	end)

	it("keeps its stones once baked", function(t)
		local e = t:spawn(scatter({ count = 4 }))
		e.script:bake()
		expect(e.script.props.baked).to_be(true)
		-- The built four and the baked four, until it builds again (baked: nothing).
		expect(#stones(e)).to_be(8)
	end)
end)
