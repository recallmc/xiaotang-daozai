//! 常驻组件按实际占位排布，并在开关变化时连续补位。
use crate::anim::{smoothstep, Spring};

pub(crate) const CLOCK: usize = 0;
pub(crate) const BATTERY: usize = 1;
pub(crate) const QUOTA: usize = 2;
/// 可独立排列的天气徽标。
pub(crate) const WEATHER: usize = 5;
/// 订阅流量徽标：**独立槽位**，固定排在额度环之后。
///
/// 早先把它当成一个「额度环」塞进额度组（`quotas = 额度数 + 1`），再用 `widen`
/// 补文字宽度 —— 这会同时做两件错事：额度组按环宽 27/间距 34 重排（环被撑开），
/// 而且文字宽度是在已经加宽的组上再加一遍，整条常驻条被白白撑长一大截，
/// 于是岛体变长、两端内容贴边。用户单独关掉订阅就正常，正是这个原因。
pub(crate) const SUBSCRIPTION: usize = 10;
/// 订阅徽标的最小宽度：图标 24 + 图标与文字的间距 3。
pub(crate) const SUBSCRIPTION_WIDTH: f32 = 27.0;
pub(crate) fn quota_slot(position:usize)->usize {
    if position<3 {QUOTA+position} else {SUBSCRIPTION+1+position-3}
}
pub(crate) const MUSIC: usize = 6;
pub(crate) const FOCUS: usize = 7;
pub(crate) const BELL: usize = 8;
pub(crate) const PET: usize = 9;
/// 天气徽标的基础槽宽；温度文字更长时由 `widen` 补足。
pub(crate) const WEATHER_WIDTH: f32 = 38.0;
const WIDTHS: [f32; 10] = [44.0,20.0,27.0,27.0,27.0,WEATHER_WIDTH,14.0,crate::focus_bar::WIDTH,20.0,32.0];

pub(crate) const DEFAULT_ORDER: [usize;7]=[BATTERY,PET,CLOCK,WEATHER,QUOTA,MUSIC,FOCUS];
pub(crate) fn valid_order(order:&[usize])->bool {
    order.len()==DEFAULT_ORDER.len() && DEFAULT_ORDER.iter().all(|slot|order.iter().filter(|value|*value==slot).count()==1)
}

pub(crate) fn animations_enabled()->bool {
    #[cfg(windows)] {
        static ENABLED:std::sync::OnceLock<bool>=std::sync::OnceLock::new();
        *ENABLED.get_or_init(||{
            use windows::Win32::{Foundation::BOOL,UI::WindowsAndMessaging::*};
            let mut enabled=BOOL(1);
            unsafe {let _=SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION,0,Some((&mut enabled as *mut BOOL).cast()),SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));}
            enabled.as_bool()
        })
    }
    #[cfg(not(windows))] {true}
}

#[derive(Clone, Debug)]
pub(crate) struct Layout {
    pub width: f32,
    pub x: Vec<Option<f32>>,
    pub widths:Vec<f32>,
    quotas:usize,
}

impl Layout {
    pub fn new(battery:bool,quotas:usize,music:bool,focus:bool,bell:bool,pet:bool,weather:bool,subscription:bool)->Self {
        fn place(cursor:&mut f32,width:f32)->f32 {let x=*cursor+width/2.0;*cursor+=width+12.0;x}
        let extra_quotas=quotas.saturating_sub(3);
        let mut widths=WIDTHS.to_vec();
        widths.resize((WIDTHS.len()+extra_quotas).max(SUBSCRIPTION+1+extra_quotas),27.0);
        let mut x=vec![None;widths.len()];let mut cursor=16.0;
        let count=quotas;
        let order=if battery {[BATTERY,CLOCK,QUOTA]}else{[QUOTA,CLOCK,BATTERY]};
        for slot in order {
            match slot {
                BATTERY if battery=>x[slot]=Some(place(&mut cursor,WIDTHS[slot])),
                CLOCK=>{
                    if pet {x[PET]=Some(place(&mut cursor,WIDTHS[PET]));}
                    x[slot]=Some(place(&mut cursor,WIDTHS[slot]));
                    if weather {x[WEATHER]=Some(place(&mut cursor,WIDTHS[WEATHER]));}
                },
                QUOTA if count>0=>{
                    let width=count as f32*27.0+(count-1) as f32*7.0;
                    let left=place(&mut cursor,width)-width/2.0;
                    for i in 0..count {x[quota_slot(i)]=Some(left+13.5+i as f32*34.0);}
                },
                _=>{},
            }
        }
        if subscription {x[SUBSCRIPTION]=Some(place(&mut cursor,SUBSCRIPTION_WIDTH));}
        for (slot,on) in [(MUSIC,music&&!bell),(FOCUS,focus),(BELL,bell)] {
            if on {x[slot]=Some(place(&mut cursor,WIDTHS[slot]));}
        }
        let width=cursor-12.0+16.0;
        for value in x.iter_mut().flatten() {*value-=width/2.0;}
        Self {width:width.max(96.0),x,widths,quotas}
    }
    pub fn quota_slots(&self)->impl Iterator<Item=usize> { (0..self.quotas).map(quota_slot) }
    /// 把某个槽加宽 extra：其余槽对称让位并记入总宽。点数文字、时段时钟、天气温度共用。
    pub fn widen(&mut self,slot:usize,extra:f32){
        if extra<=0.0 {return;}
        let Some(center)=self.x.get(slot).copied().flatten() else {return;};
        for (i,x) in self.x.iter_mut().enumerate(){if i!=slot{if let Some(x)=x{*x+=if *x<center{-extra/2.0}else{extra/2.0};}}}
        self.widths[slot]+=extra;self.width+=extra;
    }
    pub fn balance_tail(&mut self,position:usize,extra:f32){self.widen(quota_slot(position),extra);}
    pub fn widen_clock(&mut self, extra:f32) {self.widen(CLOCK,extra);}
    pub fn hide_clock(&mut self) {
        let Some(center)=self.x[CLOCK].take() else{return;};
        let removed=self.widths[CLOCK]+12.0;
        self.width=(self.width-removed).max(96.0);
        for x in self.x.iter_mut().flatten() {*x+=if *x<center {removed/2.0}else{-removed/2.0};}
    }
    pub fn reorder(&mut self,order:&[usize]) {
        if !valid_order(order){return;}
        let original=self.x.clone();let mut cursor=-self.width/2.0+16.0;
        for slot in order.iter().copied().chain(std::iter::once(BELL)) {
            // 订阅徽标固定在额度组里，跟着额度环一起换位置，不会被留在原地。
            let slots:Vec<usize>=if slot==QUOTA{
                let mut slots:Vec<usize>=self.quota_slots().collect();
                if self.x.get(SUBSCRIPTION).copied().flatten().is_some() {slots.push(SUBSCRIPTION);}
                slots
            }else{vec![slot]};
            let left=slots.iter().filter_map(|&i|original[i].map(|x|x-self.widths[i]/2.0)).fold(f32::INFINITY,f32::min);
            let right=slots.iter().filter_map(|&i|original[i].map(|x|x+self.widths[i]/2.0)).fold(f32::NEG_INFINITY,f32::max);
            if !left.is_finite(){continue;}
            for i in slots {if let Some(x)=original[i]{self.x[i]=Some(cursor+x-left);}}
            cursor+=right-left+12.0;
        }
    }
    /// 圆环换边经过时钟时，暂时淡出时钟，避免移动中相互遮挡。
    pub fn clearance(&self,slot:usize)->f32 {
        let Some(x)=self.x[slot] else{return 0.0;};
        let distance=self.quota_slots().filter_map(|i|self.x[i].map(|other|(other-x).abs()-(self.widths[i]-27.0)/2.0)).fold(f32::INFINITY,f32::min);
        let quota=if slot==CLOCK {smoothstep(34.0,44.0,distance)}else{smoothstep(22.0,34.0,distance)};
        quota*self.pet_clearance(slot)
    }
    pub fn pet_clearance(&self,slot:usize)->f32 {
        match (self.x[slot],self.x[PET]) {
            (Some(x),Some(pet))=>smoothstep(self.widths[slot]/2.0+14.0,self.widths[slot]/2.0+24.0,(x-pet).abs()),
            _=>1.0,
        }
    }
}

pub(crate) struct Motion {
    pub target: Layout,
    positions: Vec<Spring>,
    initialized: bool,
}
impl Default for Motion {
    fn default()->Self {Self {target:Layout::new(false,0,false,false,false,false,false,false),positions:vec![Spring::new(0.0,360.0,0.76);10],initialized:false}}
}
impl Motion {
    pub fn step(&mut self,target:Layout,dt:f32,animate:bool) {
        self.positions.resize(target.x.len(),Spring::new(0.0,360.0,0.76));
        for (i,x) in target.x.iter().enumerate() {
            if let Some(x)=x {
                let spring=&mut self.positions[i];spring.target=*x;
                if !self.initialized || self.target.x.get(i).copied().flatten().is_none() || !animate {spring.snap(*x);}else{spring.step(dt);}
            }
        }
        self.target=target;self.initialized=true;
    }
    pub fn current(&self)->Layout {
        let mut layout=self.target.clone();
        for (i,x) in layout.x.iter_mut().enumerate() {if x.is_some(){*x=Some(self.positions[i].pos);}}
        layout
    }
    pub fn position(&self,slot:usize)->f32 {self.positions[slot].pos}
    pub fn settled(&self)->bool {self.target.x.iter().enumerate().all(|(i,x)|x.is_none()||self.positions[i].settled(100.0))}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hidden_clock_closes_its_gap_in_custom_orders(){
        for reverse in [false,true] {for count in [0,5] {
            let mut order=DEFAULT_ORDER.to_vec();if reverse {order.reverse();}
            let mut layout=Layout::new(true,count,true,true,false,true,true,false);
            layout.reorder(&order);layout.widen_clock(64.0);
            let width=layout.width;let removed=layout.widths[CLOCK]+12.0;
            layout.hide_clock();assert!(layout.x[CLOCK].is_none());
            assert_eq!(layout.width,(width-removed).max(96.0));
            let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|(x-layout.widths[i]/2.0,x+layout.widths[i]/2.0))).collect();
            boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
            assert!(boxes.windows(2).all(|p|p[1].0-p[0].1>=6.9),"关闭时间后相邻组件不应重叠");
            let after=layout.x.clone();layout.hide_clock();assert_eq!(layout.x,after);
        }}
        let mut empty=Layout::new(false,0,false,false,false,false,false,false);
        empty.hide_clock();assert_eq!(empty.width,96.0);assert!(empty.x.iter().all(Option::is_none));
    }
    /// 订阅徽标必须是**独立槽位**：不改变额度环的间距、紧接在额度环右侧、
    /// 不与任何组件重叠，而且整条宽度的增量正好等于它自己补的那点宽度。
    ///
    /// 之前把它当成一个额度环塞进额度组（quotas+1）再 widen，会同时撑开环间距
    /// 并二次加宽 —— 常驻条被白白撑长一大截，岛体变长、两端内容贴边。
    /// 用户单独关掉订阅就正常，正是这个原因。
    #[test]
    fn subscription_slot_does_not_disturb_the_quota_rings() {
        for quotas in 0..=5 {
            let mut with = Layout::new(true, quotas, true, true, false, true, true, true);
            with.widen(SUBSCRIPTION, 40.0);
            let without = Layout::new(true, quotas, true, true, false, true, true, false);
            if quotas >= 2 {
                let pitch = |l: &Layout| l.x[quota_slot(1)].unwrap() - l.x[quota_slot(0)].unwrap();
                assert!((pitch(&with) - pitch(&without)).abs() < 0.01, "{quotas} 个额度环的间距被订阅撑开了：{} vs {}", pitch(&with), pitch(&without));
            }
            let sub = with.x[SUBSCRIPTION].expect("订阅槽位应存在");
            let half = with.widths[SUBSCRIPTION] / 2.0;
            // 紧接在额度环右侧
            if quotas > 0 {
                let ring_right = with.quota_slots().filter_map(|i| with.x[i].map(|x| x + with.widths[i] / 2.0)).fold(f32::NEG_INFINITY, f32::max);
                assert!(sub - half >= ring_right - 0.01, "订阅徽标应排在额度环之后：左缘 {} < 环右缘 {}", sub - half, ring_right);
            }
            // 不与任何其它槽位重叠
            for (i, x) in with.x.iter().enumerate() {
                if i == SUBSCRIPTION { continue; }
                if let Some(x) = x {
                    let (l, r) = (x - with.widths[i] / 2.0, x + with.widths[i] / 2.0);
                    assert!(sub + half <= l + 0.01 || sub - half >= r - 0.01, "订阅徽标与槽位 {i} 重叠：{}", sub);
                }
            }
            // 宽度增量 = 槽宽 + 组件间距 + 补宽，不能多算一遍
            let want = SUBSCRIPTION_WIDTH + 12.0 + 40.0;
            assert!((with.width - without.width - want).abs() < 0.01, "宽度增量应等于槽宽+间距+补宽（{want}），实际 {}：多算了一轮", with.width - without.width);
            // 换排列顺序后订阅仍跟着额度组，不会落在原地
            let mut moved = Layout::new(true, quotas, true, true, false, true, true, true);
            moved.reorder(&[FOCUS, MUSIC, QUOTA, CLOCK, PET, BATTERY, WEATHER]);
            let rings_left = moved.quota_slots().filter_map(|i| moved.x[i].map(|x| x - moved.widths[i] / 2.0)).fold(f32::INFINITY, f32::min);
            let rings_right = moved.quota_slots().filter_map(|i| moved.x[i].map(|x| x + moved.widths[i] / 2.0)).fold(f32::NEG_INFINITY, f32::max);
            let sub = moved.x[SUBSCRIPTION].expect("重排后订阅槽位不应丢失");
            assert!(sub - moved.widths[SUBSCRIPTION] / 2.0 >= rings_right - 0.01 && sub + moved.widths[SUBSCRIPTION] / 2.0 <= rings_left + moved.width + 0.01, "重排后订阅徽标没跟着额度组");
        }
    }

    #[test]
    fn weather_can_be_reordered_independently_of_the_clock(){
        let order=[WEATHER,FOCUS,MUSIC,QUOTA,CLOCK,PET,BATTERY];
        assert!(valid_order(&order),"天气应有独立的排列位置");
        for quotas in 0..=5 {for enabled in [false,true] {
            let mut layout=Layout::new(true,quotas,true,true,false,true,enabled,false);
            layout.reorder(&order);layout.widen(WEATHER,18.0);layout.widen_clock(40.0);
            let positions:Vec<_>=order.iter().filter_map(|slot|layout.x[*slot]).collect();
            assert!(positions.windows(2).all(|pair|pair[0]<pair[1]));
            let boxes:Vec<_>=order.iter().flat_map(|&slot|if slot==QUOTA{layout.quota_slots().collect()}else{vec![slot]}).filter_map(|slot|layout.x[slot].map(|x|(x-layout.widths[slot]/2.0,x+layout.widths[slot]/2.0))).collect();
            assert!(boxes.windows(2).all(|pair|pair[1].0-pair[0].1>=6.9),"排序和加宽后组件不能重叠");
        }}
    }
    #[test] fn deepseek_balance_follows_more_and_never_overlaps(){
        for order in [DEFAULT_ORDER,[FOCUS,MUSIC,QUOTA,CLOCK,WEATHER,PET,BATTERY]] {for quotas in 1..=6 {for battery in [false,true]{
            let mut layout=Layout::new(battery,quotas,true,true,false,true,false,false);layout.reorder(&order);layout.balance_tail(quotas-1,76.0);layout.widen_clock(40.0);
            let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|{let width=layout.widths[i];(x-width/2.0,x+width/2.0)})).collect();boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
            assert!(boxes.windows(2).all(|p|p[1].0-p[0].1>=6.9));assert!(boxes.last().unwrap().1<=layout.width/2.0-15.9);
            assert!(layout.width<crate::app::WIN_W);
        }}}
    }
    #[test]
    fn clock_12h_keeps_components_separate_in_custom_orders(){
        for order in [DEFAULT_ORDER,[FOCUS,MUSIC,QUOTA,CLOCK,WEATHER,PET,BATTERY]] {
            for quotas in 0..=5 {
                let mut layout=Layout::new(true,quotas,true,true,false,true,false,false);
                layout.reorder(&order);layout.widen_clock(40.0);
                let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|{
                    let width=layout.widths[i];
                    (x-width/2.0,x+width/2.0)
                })).collect();
                boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
                assert!(boxes.windows(2).all(|p|p[1].0-p[0].1>=6.9));
                assert!(boxes.first().unwrap().0>=-layout.width/2.0+15.9);
                assert!(boxes.last().unwrap().1<=layout.width/2.0-15.9);
            }
        }
    }
    #[test]
    fn custom_order_preserves_groups_and_hidden_positions(){
        assert!(!valid_order(&[0,0,2,6,7,9]));
        assert!(!valid_order(&[0,1,2]));
        let order=[FOCUS,MUSIC,QUOTA,CLOCK,WEATHER,PET,BATTERY];
        for battery in [false,true]{for count in 0..=5 {
            let mut layout=Layout::new(battery,count,true,true,false,true,false,false);
            let width=layout.width;layout.reorder(&order);
            assert_eq!(layout.width,width);
            let positions:Vec<_>=order.iter().filter_map(|i|layout.x[*i]).collect();
            assert!(positions.windows(2).all(|p|p[0]<p[1]));
            if count>1 {assert_eq!(layout.x[QUOTA+1].unwrap()-layout.x[QUOTA].unwrap(),34.0);}
            let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|(x-layout.widths[i]/2.0,x+layout.widths[i]/2.0))).collect();
            boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
            assert!(boxes.windows(2).all(|p|p[1].0-p[0].1>=6.9));
        }}
    }
    #[test]
    fn every_component_combination_fits_without_overlaps(){
        for quotas in 0..=8 {for battery in [false,true] {for music in [false,true] {for focus in [false,true] {for bell in [false,true] {for pet in [false,true] {for weather in [false,true] {
            let layout=Layout::new(battery,quotas,music,focus,bell,pet,weather,false);
            let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|(x-layout.widths[i]/2.0,x+layout.widths[i]/2.0))).collect();
            boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
            assert_eq!(layout.quota_slots().count(),quotas);
            assert!(boxes.first().unwrap().0>=-layout.width/2.0+15.9);
            assert!(boxes.last().unwrap().1<=layout.width/2.0-15.9);
            assert!(boxes.windows(2).all(|pair|pair[1].0-pair[0].1>=6.9),"组件不能挤在一起：{layout:?}");
            assert_eq!(layout.clearance(CLOCK),1.0);
        }}}}}}}
    }
    #[test]
    fn weather_defaults_after_the_clock_and_preserves_custom_spacing(){
        for order in [DEFAULT_ORDER,[FOCUS,MUSIC,QUOTA,CLOCK,WEATHER,PET,BATTERY],[QUOTA,MUSIC,CLOCK,WEATHER,PET,FOCUS,BATTERY]] {
            for quotas in 0..=3 {for battery in [false,true] {
                let mut layout=Layout::new(battery,quotas,true,true,false,true,true,false);
                layout.reorder(&order);
                let (clock,weather)=match (layout.x[CLOCK],layout.x[WEATHER]) {(Some(c),Some(w))=>(c,w),_=>panic!("时钟与天气都应就位：{layout:?}")};
                assert!(weather>clock+22.0+6.9,"天气应紧跟时钟右侧：时钟 {clock}，天气 {weather}");
                // 相邻排列的时钟与天气保持标准组件间距。
                assert!((weather-clock-(38.0+44.0)/2.0-12.0).abs()<0.01);
                let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|(x-layout.widths[i]/2.0,x+layout.widths[i]/2.0))).collect();
                boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
                assert!(boxes.windows(2).all(|p|p[1].0-p[0].1>=6.9));
                assert!(boxes.last().unwrap().1<=layout.width/2.0-15.9);
                assert!(layout.clearance(WEATHER)>0.99,"天气与额度环不应对撞：{layout:?}");
            }}
        }
    }
    #[test]
    fn weather_chip_widens_symmetrically_without_overlaps(){
        let mut layout=Layout::new(true,3,true,true,false,true,true,false);
        let before=layout.x.clone();
        layout.widen(WEATHER,18.0);
        assert!((layout.widths[WEATHER]-56.0).abs()<0.01);
        for (slot,x) in layout.x.iter().enumerate() {
            match (slot,x) {
                (crate::bar_layout::WEATHER,Some(_))=>{},
                (_,Some(x))=>assert!((*x-before[slot].unwrap()).abs()>8.0,"其余槽位应被推开：{slot}"),
                (_,None)=>{},
            }
        }
        let mut boxes:Vec<_>=layout.x.iter().enumerate().filter_map(|(i,x)|x.map(|x|(x-layout.widths[i]/2.0,x+layout.widths[i]/2.0))).collect();
        boxes.sort_by(|a,b|a.0.total_cmp(&b.0));
        assert!(boxes.windows(2).all(|p|p[1].0-p[0].1>=6.9));
        // 未就位的槽位调用 widen 应是空操作。
        let mut empty=Layout::new(true,0,true,true,false,true,false,false);
        let width=empty.width;empty.widen(WEATHER,20.0);
        assert_eq!(empty.width,width);
    }
    #[test]
    fn replacing_battery_moves_continuously_and_supports_an_instant_state(){
        let before=Layout::new(true,1,true,false,false,true,false,false);let after=Layout::new(false,1,true,false,false,true,false,false);
        let mut motion=Motion::default();motion.step(before.clone(),0.0,true);
        motion.step(after.clone(),1.0/60.0,true);let mid=motion.current().x[QUOTA].unwrap();
        assert!(mid<before.x[QUOTA].unwrap()&&mid>after.x[QUOTA].unwrap());
        for _ in 0..120 {motion.step(after.clone(),1.0/60.0,true);}
        assert!((motion.current().x[QUOTA].unwrap()-after.x[QUOTA].unwrap()).abs()<0.01);
        motion.step(before.clone(),1.0/60.0,false);assert_eq!(motion.current().x,before.x);
    }
}
